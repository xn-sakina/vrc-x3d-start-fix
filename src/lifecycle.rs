use serde::{Deserialize, Serialize};

use crate::{config::DisturbanceParams, heuristic::AttemptOutcome};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    ProtectedLauncher,
    EasyAntiCheat,
    VrchatProcess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerInfo {
    pub source: TriggerSource,
    pub source_pid: u32,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    CandidateDetected {
        trigger: TriggerInfo,
    },
    Disturbing {
        attempt_id: u64,
        params: DisturbanceParams,
    },
    CoolingDown {
        pid: Option<u32>,
        outcome: AttemptOutcome,
    },
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    #[test]
    fn state_payload_keeps_an_attempt_parameter_snapshot() {
        let state = AppState::Disturbing {
            attempt_id: 7,
            params: Profile::Robust.params(),
        };
        if let AppState::Disturbing { params, .. } = state {
            assert_eq!(params.duty_percent, 50);
        } else {
            panic!("wrong state");
        }
    }
}
