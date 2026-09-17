use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::config::{DisturbanceParams, WorkerPriority};

use super::WorkerPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerExit {
    pub index: usize,
    pub affinity_set: bool,
    pub priority_set: bool,
    pub iterations: u64,
}

pub fn run_worker(
    plan: WorkerPlan,
    params: DisturbanceParams,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) -> WorkerExit {
    let affinity_set = plan.core_id.is_none_or(core_affinity::set_for_current);
    let priority_set = set_priority(params.priority);
    cancellable_sleep(plan.phase, &cancel, deadline);

    let busy = Duration::from_millis(params.busy_ms());
    let rest = Duration::from_millis(params.rest_ms());
    let mut value = (plan.index as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut iterations = 0u64;

    while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
        let busy_until = Instant::now()
            .checked_add(busy)
            .unwrap_or(deadline)
            .min(deadline);
        while Instant::now() < busy_until {
            value = value
                .rotate_left(7)
                .wrapping_mul(0x5851_f42d_4c95_7f2d)
                .wrapping_add(1);
            iterations = iterations.wrapping_add(1);
            if iterations & 1023 == 0 && cancel.load(Ordering::Acquire) {
                break;
            }
        }
        black_box(value);
        cancellable_sleep(rest, &cancel, deadline);
    }

    WorkerExit {
        index: plan.index,
        affinity_set,
        priority_set,
        iterations,
    }
}

fn cancellable_sleep(duration: Duration, cancel: &AtomicBool, deadline: Instant) {
    let end = Instant::now()
        .checked_add(duration)
        .unwrap_or(deadline)
        .min(deadline);
    while !cancel.load(Ordering::Acquire) {
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

#[cfg(target_os = "windows")]
fn set_priority(priority: WorkerPriority) -> bool {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    match priority {
        WorkerPriority::BelowNormal => unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL).is_ok()
        },
    }
}

#[cfg(not(target_os = "windows"))]
fn set_priority(_priority: WorkerPriority) -> bool {
    true
}
