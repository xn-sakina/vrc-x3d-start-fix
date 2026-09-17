use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::config::DisturbanceParams;

use super::{WorkerExit, build_worker_plan, worker::run_worker};

#[derive(Debug, Default)]
pub struct DisturbanceReport {
    pub worker_count: usize,
    pub affinity_successes: usize,
    pub affinity_failures: usize,
    pub priority_successes: usize,
    pub priority_failures: usize,
    pub panics: usize,
    pub elapsed: Duration,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("no logical processors were reported")]
    NoLogicalProcessors,
    #[error("failed to create worker {index}: {source}")]
    Spawn {
        index: usize,
        #[source]
        source: std::io::Error,
    },
}

pub struct DisturbanceSession {
    cancel: Arc<AtomicBool>,
    workers: Vec<JoinHandle<WorkerExit>>,
    started_at: Instant,
    deadline: Instant,
    params: DisturbanceParams,
}

impl DisturbanceSession {
    pub fn start(
        params: DisturbanceParams,
        physical_core_count: usize,
    ) -> Result<Self, SessionError> {
        let logical_cores =
            core_affinity::get_core_ids().ok_or(SessionError::NoLogicalProcessors)?;
        if logical_cores.is_empty() {
            return Err(SessionError::NoLogicalProcessors);
        }
        let plans = build_worker_plan(&params, &logical_cores, physical_core_count);
        let cancel = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();
        let deadline = started_at + Duration::from_secs(params.hard_stop_secs);
        let mut workers = Vec::with_capacity(plans.len());

        for plan in plans {
            let worker_cancel = Arc::clone(&cancel);
            let worker_params = params.clone();
            match thread::Builder::new()
                .name(format!("disturbance-{}", plan.index))
                .spawn(move || run_worker(plan, worker_params, worker_cancel, deadline))
            {
                Ok(handle) => workers.push(handle),
                Err(source) => {
                    cancel.store(true, Ordering::Release);
                    for handle in workers {
                        let _ = handle.join();
                    }
                    return Err(SessionError::Spawn {
                        index: plan.index,
                        source,
                    });
                }
            }
        }

        Ok(Self {
            cancel,
            workers,
            started_at,
            deadline,
            params,
        })
    }

    pub fn params(&self) -> &DisturbanceParams {
        &self.params
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn stop(&mut self) -> DisturbanceReport {
        self.cancel.store(true, Ordering::Release);
        let mut report = DisturbanceReport {
            worker_count: self.workers.len(),
            elapsed: self.started_at.elapsed(),
            ..DisturbanceReport::default()
        };
        for handle in self.workers.drain(..) {
            match handle.join() {
                Ok(exit) => {
                    report.affinity_successes += usize::from(exit.affinity_set);
                    report.affinity_failures += usize::from(!exit.affinity_set);
                    report.priority_successes += usize::from(exit.priority_set);
                    report.priority_failures += usize::from(!exit.priority_set);
                }
                Err(_) => report.panics += 1,
            }
        }
        report.elapsed = self.started_at.elapsed();
        report
    }
}

impl Drop for DisturbanceSession {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            let _ = self.stop();
        }
    }
}
