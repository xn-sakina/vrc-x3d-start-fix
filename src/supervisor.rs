use std::{
    collections::HashSet,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde_json::json;

use crate::{
    config::{AppConfig, ConfigStore, CpuCoverage, Profile},
    disturbance::DisturbanceSession,
    heuristic::{AttemptOutcome, AttemptTracker},
    lifecycle::TriggerInfo,
    monitor::{ProcessMonitor, ProcessSnapshot, TriggerCandidate},
};

const IDLE_POLL: Duration = Duration::from_millis(250);
const ACTIVE_POLL: Duration = Duration::from_millis(100);
const CPU_LOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub enum SupervisorCommand {
    SelectProfile(Profile),
    SetDuty(u8),
    SetDuration(u64),
    SetCoverage(CpuCoverage),
    ResetDefaults,
    OpenLogFolder,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiStatus {
    Waiting,
    Detected,
    Disturbing,
    Success,
    Failed,
    Unknown,
    ShuttingDown,
    ShutdownComplete,
}

impl UiStatus {
    pub fn locale_key(&self) -> &'static str {
        match self {
            Self::Waiting => "status.waiting",
            Self::Detected => "status.detected",
            Self::Disturbing => "status.disturbing",
            Self::Success => "status.success",
            Self::Failed => "status.failed",
            Self::Unknown => "status.unknown",
            Self::ShuttingDown | Self::ShutdownComplete => "status.shutting_down",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UiUpdate {
    pub status: UiStatus,
    pub config: AppConfig,
}

struct ActiveAttempt {
    id: u64,
    trigger: TriggerInfo,
    params: crate::config::DisturbanceParams,
    session: DisturbanceSession,
    vrchat: Option<ProcessSnapshot>,
    bound_at: Option<Instant>,
    tracker: Option<AttemptTracker>,
    late_attach: bool,
    last_cpu_log: Instant,
}

pub fn spawn(
    config: AppConfig,
    store: ConfigStore,
    log_dir: PathBuf,
    command_rx: Receiver<SupervisorCommand>,
    update_tx: Sender<UiUpdate>,
    shutdown_done_tx: Sender<()>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("supervisor".into())
        .spawn(move || {
            run(
                config,
                store,
                log_dir,
                command_rx,
                update_tx,
                shutdown_done_tx,
            )
        })
        .expect("create supervisor thread")
}

fn run(
    mut config: AppConfig,
    store: ConfigStore,
    log_dir: PathBuf,
    command_rx: Receiver<SupervisorCommand>,
    update_tx: Sender<UiUpdate>,
    shutdown_done_tx: Sender<()>,
) {
    let mut monitor = ProcessMonitor::new();
    monitor.refresh(false);
    let physical_cores = monitor.physical_core_count();
    let logical_processors = monitor.logical_processor_count();
    tracing::info!(
        event = "app_start",
        version = env!("CARGO_PKG_VERSION"),
        os_version = %monitor.os_version(),
        cpu_brand = %monitor.cpu_brand(),
        physical_cores,
        logical_processors,
        "supervisor started"
    );

    let mut status = UiStatus::Waiting;
    send_update(&update_tx, &status, &config);
    let mut ignored_pids = HashSet::new();
    let mut active: Option<ActiveAttempt> = None;
    let mut cooldown: Option<(Option<u32>, Instant)> = None;
    let mut next_attempt_id = 1u64;
    let mut last_poll = Instant::now()
        .checked_sub(IDLE_POLL)
        .unwrap_or_else(Instant::now);
    let mut first_scan = true;

    'supervisor: loop {
        let interval = if active.is_some() {
            ACTIVE_POLL
        } else {
            IDLE_POLL
        };
        let wait = interval.saturating_sub(last_poll.elapsed());
        match command_rx.recv_timeout(wait) {
            Ok(SupervisorCommand::Shutdown) => {
                status = UiStatus::ShuttingDown;
                send_update(&update_tx, &status, &config);
                if let Some(mut attempt) = active.take() {
                    finish_attempt(
                        &mut attempt,
                        AttemptOutcome::UnknownTimeout,
                        &mut ignored_pids,
                    );
                }
                if let Err(error) = store.save(&config) {
                    tracing::error!(event = "config_save_failed", error = %error);
                }
                tracing::info!(event = "app_shutdown", "supervisor stopped");
                send_update(&update_tx, &UiStatus::ShutdownComplete, &config);
                let _ = shutdown_done_tx.try_send(());
                break 'supervisor;
            }
            Ok(SupervisorCommand::OpenLogFolder) => {
                if let Err(error) = open::that(&log_dir) {
                    tracing::error!(event = "open_log_folder_failed", error = %error, path = %log_dir.display());
                }
                continue;
            }
            Ok(command) => {
                if apply_config_command(&mut config, command) {
                    match store.save(&config) {
                        Ok(()) => tracing::info!(
                            event = "config_updated",
                            profile = ?config.profile,
                            params = %json!(config.resolved_params())
                        ),
                        Err(error) => {
                            tracing::error!(event = "config_save_failed", error = %error)
                        }
                    }
                    send_update(&update_tx, &status, &config);
                }
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut attempt) = active.take() {
                    finish_attempt(
                        &mut attempt,
                        AttemptOutcome::InternalError,
                        &mut ignored_pids,
                    );
                }
                tracing::warn!(event = "command_channel_disconnected");
                let _ = shutdown_done_tx.try_send(());
                break 'supervisor;
            }
        }
        last_poll = Instant::now();
        monitor.refresh(active.is_some());
        ignored_pids.retain(|pid| monitor.pid_exists(*pid));

        if let Some((pid, since)) = cooldown {
            let complete = pid.is_none_or(|value| !monitor.pid_exists(value))
                || since.elapsed() >= Duration::from_secs(5);
            if complete {
                cooldown = None;
                status = UiStatus::Waiting;
                send_update(&update_tx, &status, &config);
            } else {
                continue;
            }
        }

        if let Some(attempt) = active.as_mut() {
            if attempt.vrchat.is_none() {
                if let Some(process) = monitor.find_vrchat(&ignored_pids) {
                    bind_vrchat(attempt, process);
                }
            }

            let mut outcome = if let (Some(process), Some(bound_at), Some(tracker)) =
                (&attempt.vrchat, attempt.bound_at, attempt.tracker.as_mut())
            {
                let observation = monitor.observation(process.pid);
                tracker.observe(bound_at.elapsed(), &observation)
            } else if Instant::now() >= attempt.session.deadline() {
                Some(AttemptOutcome::UnknownTimeout)
            } else {
                None
            };
            if outcome.is_none() && Instant::now() >= attempt.session.deadline() {
                outcome = Some(AttemptOutcome::UnknownTimeout);
            }

            if attempt.last_cpu_log.elapsed() >= CPU_LOG_INTERVAL {
                attempt.last_cpu_log = Instant::now();
                let metrics =
                    monitor.cpu_metrics(attempt.vrchat.as_ref().map(|process| process.pid));
                tracing::info!(
                    event = "attempt_cpu_sample",
                    attempt_id = attempt.id,
                    system_cpu_percent = metrics.system_total,
                    logical_cpu_min = metrics.logical_min,
                    logical_cpu_median = metrics.logical_median,
                    logical_cpu_max = metrics.logical_max,
                    vrchat_cpu_percent = metrics.vrchat,
                );
            }

            if let Some(outcome) = outcome {
                let mut attempt = active.take().expect("active attempt exists");
                let pid = attempt.vrchat.as_ref().map(|process| process.pid);
                let final_outcome = finish_attempt(&mut attempt, outcome, &mut ignored_pids);
                status = match final_outcome {
                    AttemptOutcome::SuccessHeuristic => UiStatus::Success,
                    AttemptOutcome::UnknownTimeout => UiStatus::Unknown,
                    _ => UiStatus::Failed,
                };
                send_update(&update_tx, &status, &config);
                cooldown = Some((pid, Instant::now()));
            }
            continue;
        }

        if let Some(candidate) = monitor.find_trigger(&ignored_pids) {
            if first_scan {
                if let Some(vrchat) = &candidate.vrchat {
                    if vrchat.run_time_secs > 15 {
                        ignored_pids.insert(vrchat.pid);
                        ignored_pids.insert(candidate.trigger.source_pid);
                        tracing::info!(
                            event = "already_running",
                            vrchat_pid = vrchat.pid,
                            run_time_secs = vrchat.run_time_secs
                        );
                        first_scan = false;
                        continue;
                    }
                }
            }
            first_scan = false;
            status = UiStatus::Detected;
            send_update(&update_tx, &status, &config);
            ignored_pids.insert(candidate.trigger.source_pid);
            match start_attempt(
                next_attempt_id,
                candidate,
                config.resolved_params(),
                physical_cores,
            ) {
                Ok(attempt) => {
                    next_attempt_id += 1;
                    status = UiStatus::Disturbing;
                    send_update(&update_tx, &status, &config);
                    active = Some(attempt);
                }
                Err(error) => {
                    tracing::error!(event = "attempt_start_failed", error = %error);
                    status = UiStatus::Failed;
                    send_update(&update_tx, &status, &config);
                    cooldown = Some((None, Instant::now()));
                }
            }
        } else {
            first_scan = false;
        }
    }
}

fn start_attempt(
    id: u64,
    candidate: TriggerCandidate,
    params: crate::config::DisturbanceParams,
    physical_cores: usize,
) -> Result<ActiveAttempt, crate::disturbance::SessionError> {
    let session = DisturbanceSession::start(params.clone(), physical_cores)?;
    let late_attach = candidate
        .vrchat
        .as_ref()
        .is_some_and(|process| process.run_time_secs > 0);
    let mut attempt = ActiveAttempt {
        id,
        trigger: candidate.trigger,
        params,
        session,
        vrchat: None,
        bound_at: None,
        tracker: None,
        late_attach,
        last_cpu_log: Instant::now(),
    };
    if let Some(process) = candidate.vrchat {
        bind_vrchat(&mut attempt, process);
    }
    tracing::info!(
        event = "attempt_started",
        attempt_id = attempt.id,
        app_version = env!("CARGO_PKG_VERSION"),
        config_version = crate::config::CONFIG_SCHEMA_VERSION,
        trigger = ?attempt.trigger.source,
        trigger_pid = attempt.trigger.source_pid,
        trigger_path = ?attempt.trigger.path,
        late_attach,
        profile_params = %json!(attempt.params),
        worker_count = attempt.session.worker_count(),
        "disturbance started"
    );
    Ok(attempt)
}

fn bind_vrchat(attempt: &mut ActiveAttempt, process: ProcessSnapshot) {
    let now = Instant::now();
    let tracker = AttemptTracker::new(
        Duration::from_secs(attempt.params.success_after_secs),
        Duration::from_secs(attempt.params.hard_stop_secs),
    );
    tracing::info!(
        event = "vrchat_bound",
        attempt_id = attempt.id,
        vrchat_pid = process.pid,
        vrchat_path = ?process.path,
        detection_delay_ms = attempt.session.started_at().elapsed().as_millis() as u64,
        initial_cpu_percent = process.cpu_percent,
        process_dead = process.dead,
    );
    attempt.vrchat = Some(process);
    attempt.bound_at = Some(now);
    attempt.tracker = Some(tracker);
}

fn finish_attempt(
    attempt: &mut ActiveAttempt,
    mut outcome: AttemptOutcome,
    ignored_pids: &mut HashSet<u32>,
) -> AttemptOutcome {
    let fatal_match = attempt
        .tracker
        .as_ref()
        .and_then(|tracker| tracker.fatal_match())
        .map(str::to_owned);
    let report = attempt.session.stop();
    if report.panics > 0 {
        outcome = AttemptOutcome::InternalError;
    }
    if let Some(process) = &attempt.vrchat {
        ignored_pids.insert(process.pid);
    }
    tracing::info!(
        event = "attempt_finished",
        attempt_id = attempt.id,
        outcome = ?outcome,
        reason = ?outcome,
        vrchat_pid = attempt.vrchat.as_ref().map(|process| process.pid),
        fatal_match = ?fatal_match,
        late_attach = attempt.late_attach,
        duration_ms = report.elapsed.as_millis() as u64,
        worker_count = report.worker_count,
        affinity_successes = report.affinity_successes,
        affinity_failures = report.affinity_failures,
        priority_successes = report.priority_successes,
        priority_failures = report.priority_failures,
        worker_panics = report.panics,
        params = %json!(attempt.params),
        "disturbance finished"
    );
    outcome
}

fn apply_config_command(config: &mut AppConfig, command: SupervisorCommand) -> bool {
    match command {
        SupervisorCommand::SelectProfile(profile) => config.select_profile(profile),
        SupervisorCommand::SetDuty(duty) => config.set_duty(duty),
        SupervisorCommand::SetDuration(duration) => config.set_hard_stop(duration),
        SupervisorCommand::SetCoverage(coverage) => config.set_coverage(coverage),
        SupervisorCommand::ResetDefaults => config.reset(),
        SupervisorCommand::OpenLogFolder | SupervisorCommand::Shutdown => return false,
    }
    true
}

fn send_update(tx: &Sender<UiUpdate>, status: &UiStatus, config: &AppConfig) {
    let update = UiUpdate {
        status: status.clone(),
        config: config.clone(),
    };
    let _ = tx.try_send(update);
}
