use std::collections::HashSet;

use sysinfo::{
    CpuRefreshKind, Pid, Process, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate,
    RefreshKind, System, UpdateKind,
};

use crate::{
    heuristic::Observation,
    lifecycle::{TriggerInfo, TriggerSource},
    platform::windows::process_windows::inspect_process_windows,
};

#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub path: Option<String>,
    pub run_time_secs: u64,
    pub cpu_percent: f32,
    pub dead: bool,
}

#[derive(Debug, Clone)]
pub struct TriggerCandidate {
    pub trigger: TriggerInfo,
    pub vrchat: Option<ProcessSnapshot>,
}

#[derive(Debug, Clone, Copy)]
pub struct CpuMetrics {
    pub system_total: f32,
    pub logical_min: f32,
    pub logical_median: f32,
    pub logical_max: f32,
    pub vrchat: f32,
}

pub struct ProcessMonitor {
    system: System,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        let processes = process_refresh_kind(true);
        Self {
            system: System::new_with_specifics(
                RefreshKind::new()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_processes(processes),
            ),
        }
    }

    pub fn refresh(&mut self, include_cpu: bool) {
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, process_refresh_kind(include_cpu));
        if include_cpu {
            self.system.refresh_cpu_usage();
        }
    }

    pub fn physical_core_count(&self) -> usize {
        self.system
            .physical_core_count()
            .unwrap_or_else(|| self.system.cpus().len())
            .max(1)
    }

    pub fn logical_processor_count(&self) -> usize {
        self.system.cpus().len().max(1)
    }

    pub fn cpu_brand(&self) -> String {
        self.system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().to_owned())
            .unwrap_or_default()
    }

    pub fn os_version(&self) -> String {
        System::long_os_version().unwrap_or_else(|| "unknown".to_owned())
    }

    pub fn pid_exists(&self, pid: u32) -> bool {
        self.system.process(Pid::from_u32(pid)).is_some()
    }

    pub fn vrchat_pids(&self) -> HashSet<u32> {
        self.system
            .processes()
            .iter()
            .filter_map(|(pid, process)| is_name(process, "vrchat.exe").then_some(pid.as_u32()))
            .collect()
    }

    pub fn find_vrchat(&self, ignored: &HashSet<u32>) -> Option<ProcessSnapshot> {
        self.system.processes().iter().find_map(|(pid, process)| {
            let pid = pid.as_u32();
            (is_name(process, "vrchat.exe") && !ignored.contains(&pid))
                .then(|| snapshot(pid, process))
        })
    }

    pub fn find_trigger(&self, ignored: &HashSet<u32>) -> Option<TriggerCandidate> {
        let vrchat = self.find_vrchat(ignored);
        let launcher = self.find_named_associated("start_protected_game.exe", ignored, true);
        if let Some((pid, process)) = launcher {
            return Some(candidate(
                TriggerSource::ProtectedLauncher,
                pid,
                process,
                vrchat,
            ));
        }
        let eac = self.find_named_associated("easyanticheat_eos.exe", ignored, false);
        if let Some((pid, process)) = eac {
            return Some(candidate(
                TriggerSource::EasyAntiCheat,
                pid,
                process,
                vrchat,
            ));
        }
        vrchat.map(|process| TriggerCandidate {
            trigger: TriggerInfo {
                source: TriggerSource::VrchatProcess,
                source_pid: process.pid,
                path: process.path.clone(),
            },
            vrchat: Some(process),
        })
    }

    fn find_named_associated<'a>(
        &'a self,
        name: &str,
        ignored: &HashSet<u32>,
        path_only: bool,
    ) -> Option<(u32, &'a Process)> {
        self.system.processes().iter().find_map(|(pid, process)| {
            let pid = pid.as_u32();
            if ignored.contains(&pid) || !is_name(process, name) {
                return None;
            }
            let path = process
                .exe()
                .map(|path| path.to_string_lossy().to_ascii_lowercase());
            let cmd = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            let associated = path
                .as_deref()
                .is_some_and(|value| value.contains("vrchat"))
                || (!path_only && cmd.contains("vrchat"));
            associated.then_some((pid, process))
        })
    }

    pub fn observation(&self, pid: u32) -> Observation {
        let Some(process) = self.system.process(Pid::from_u32(pid)) else {
            return Observation::default();
        };
        let dead = matches!(
            process.status(),
            ProcessStatus::Dead | ProcessStatus::Zombie
        );
        let windows = inspect_process_windows(pid);
        Observation {
            process_exists: !dead,
            fatal_dialog: windows.fatal_text,
            werfault: self.has_associated_werfault(pid),
            visible_window: windows.visible_top_level,
            window_hung: windows.hung_top_level,
            process_cpu_percent: process.cpu_usage(),
        }
    }

    pub fn cpu_metrics(&self, vrchat_pid: Option<u32>) -> CpuMetrics {
        let mut logical = self
            .system
            .cpus()
            .iter()
            .map(|cpu| cpu.cpu_usage())
            .collect::<Vec<_>>();
        logical.sort_by(f32::total_cmp);
        let middle = logical.get(logical.len() / 2).copied().unwrap_or(0.0);
        CpuMetrics {
            system_total: self.system.global_cpu_usage(),
            logical_min: logical.first().copied().unwrap_or(0.0),
            logical_median: middle,
            logical_max: logical.last().copied().unwrap_or(0.0),
            vrchat: vrchat_pid
                .and_then(|pid| self.system.process(Pid::from_u32(pid)))
                .map(Process::cpu_usage)
                .unwrap_or(0.0),
        }
    }

    fn has_associated_werfault(&self, target_pid: u32) -> bool {
        let target = target_pid.to_string();
        self.system.processes().values().any(|process| {
            is_name(process, "werfault.exe")
                && process
                    .cmd()
                    .iter()
                    .any(|part| part.to_string_lossy().contains(&target))
        })
    }
}

fn process_refresh_kind(include_cpu: bool) -> ProcessRefreshKind {
    let kind = ProcessRefreshKind::new()
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet);
    if include_cpu { kind.with_cpu() } else { kind }
}

fn candidate(
    source: TriggerSource,
    pid: u32,
    process: &Process,
    vrchat: Option<ProcessSnapshot>,
) -> TriggerCandidate {
    TriggerCandidate {
        trigger: TriggerInfo {
            source,
            source_pid: pid,
            path: process
                .exe()
                .map(|path| path.to_string_lossy().into_owned()),
        },
        vrchat,
    }
}

fn snapshot(pid: u32, process: &Process) -> ProcessSnapshot {
    ProcessSnapshot {
        pid,
        path: process
            .exe()
            .map(|path| path.to_string_lossy().into_owned()),
        run_time_secs: process.run_time(),
        cpu_percent: process.cpu_usage(),
        dead: matches!(
            process.status(),
            ProcessStatus::Dead | ProcessStatus::Zombie
        ),
    }
}

fn is_name(process: &Process, expected: &str) -> bool {
    process
        .name()
        .to_string_lossy()
        .eq_ignore_ascii_case(expected)
}
