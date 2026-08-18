pub mod backoff;
pub mod console;
pub mod launch;
pub mod port;
pub mod signal;
pub mod supervisor;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How far the stop sequence had to go. The UI shows this so "stopped" and
/// "had to be killed" are not the same word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum StopStage {
    /// It was not running to begin with.
    AlreadyStopped,
    /// `stop` on stdin was enough.
    Graceful,
    /// stdin was ignored; SIGTERM (or `taskkill /T`) ended it.
    Terminated,
    /// It ignored everything short of SIGKILL (`taskkill /F`).
    Killed,
}

impl StopStage {
    pub fn as_str(self) -> &'static str {
        match self {
            StopStage::AlreadyStopped => "already_stopped",
            StopStage::Graceful => "graceful",
            StopStage::Terminated => "terminated",
            StopStage::Killed => "killed",
        }
    }

    /// A sentence for the console and the toast.
    pub fn describe(self, name: &str) -> String {
        match self {
            StopStage::AlreadyStopped => format!("\"{name}\" was not running"),
            StopStage::Graceful => format!("\"{name}\" stopped cleanly"),
            StopStage::Terminated => {
                format!("\"{name}\" ignored the stop command and was terminated")
            }
            StopStage::Killed => format!("\"{name}\" had to be killed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stage_describes_itself_distinctly() {
        let described: Vec<String> = [
            StopStage::AlreadyStopped,
            StopStage::Graceful,
            StopStage::Terminated,
            StopStage::Killed,
        ]
        .iter()
        .map(|stage| stage.describe("Survival"))
        .collect();

        assert!(described.iter().all(|text| text.contains("Survival")));
        let unique: std::collections::HashSet<&String> = described.iter().collect();
        assert_eq!(unique.len(), described.len(), "stages must read differently");
        assert!(described[3].contains("killed"));
    }
}
