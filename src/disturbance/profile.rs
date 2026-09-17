use std::time::Duration;

use crate::config::{CpuCoverage, DisturbanceParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPlan {
    pub index: usize,
    pub core_id: Option<core_affinity::CoreId>,
    pub phase: Duration,
}

pub fn phase_offset(index: usize, worker_count: usize, period: Duration) -> Duration {
    if worker_count == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(
        period
            .as_nanos()
            .saturating_mul(index as u128)
            .saturating_div(worker_count as u128) as u64,
    )
}

pub fn build_worker_plan(
    params: &DisturbanceParams,
    logical_cores: &[core_affinity::CoreId],
    physical_core_count: usize,
) -> Vec<WorkerPlan> {
    let count = match params.coverage {
        CpuCoverage::AllLogical | CpuCoverage::Unpinned => logical_cores.len(),
        CpuCoverage::PhysicalCores => physical_core_count.clamp(1, logical_cores.len()),
    };
    let period = Duration::from_millis(params.period_ms);
    (0..count)
        .map(|index| WorkerPlan {
            index,
            core_id: (params.coverage != CpuCoverage::Unpinned).then_some(logical_cores[index]),
            phase: phase_offset(index, count, period),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    #[test]
    fn phases_are_evenly_staggered() {
        let period = Duration::from_millis(100);
        assert_eq!(phase_offset(0, 4, period), Duration::ZERO);
        assert_eq!(phase_offset(1, 4, period), Duration::from_millis(25));
        assert_eq!(phase_offset(3, 4, period), Duration::from_millis(75));
    }

    #[test]
    fn busy_and_rest_match_duty_cycle() {
        let params = Profile::Robust.params();
        assert_eq!(params.busy_ms(), 50);
        assert_eq!(params.rest_ms(), 50);
    }
}
