use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use sqlx::SqlitePool;

use crate::db::models::{Instance, InstanceStatus, InstanceView};

/// Shared backend state. Runtime status lives here and only here — it is never
/// read back from the database as truth (see PLAN.md §2).
pub struct AppState {
    pub db: SqlitePool,
    /// Where the database, logs and the artifact cache live.
    pub data_dir: PathBuf,
    statuses: RwLock<HashMap<String, InstanceStatus>>,
}

impl AppState {
    pub fn new(db: SqlitePool, data_dir: PathBuf) -> Self {
        Self {
            db,
            data_dir,
            statuses: RwLock::new(HashMap::new()),
        }
    }

    pub fn status_of(&self, uuid: &str) -> InstanceStatus {
        self.statuses
            .read()
            .ok()
            .and_then(|m| m.get(uuid).copied())
            .unwrap_or(InstanceStatus::Stopped)
    }

    pub fn set_status(&self, uuid: &str, status: InstanceStatus) {
        if let Ok(mut map) = self.statuses.write() {
            map.insert(uuid.to_string(), status);
        }
    }

    pub fn forget(&self, uuid: &str) {
        if let Ok(mut map) = self.statuses.write() {
            map.remove(uuid);
        }
    }

    /// Instances currently alive in any form: running, starting, stopping, or an
    /// orphan we adopted at launch. Drives the quit confirmation.
    pub fn live_uuids(&self) -> Vec<String> {
        self.statuses
            .read()
            .map(|m| {
                m.iter()
                    .filter(|(_, s)| s.is_live())
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn view(&self, instance: &Instance) -> InstanceView {
        instance.to_view(self.status_of(&instance.uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state() -> AppState {
        let pool = crate::db::connect_in_memory().await.unwrap();
        AppState::new(pool, PathBuf::from("."))
    }

    #[tokio::test]
    async fn unknown_instances_default_to_stopped() {
        let s = state().await;
        assert_eq!(s.status_of("nope"), InstanceStatus::Stopped);
    }

    #[tokio::test]
    async fn live_uuids_only_lists_live_states() {
        let s = state().await;
        s.set_status("a", InstanceStatus::Running);
        s.set_status("b", InstanceStatus::Unmanaged);
        s.set_status("c", InstanceStatus::Crashed);
        s.set_status("d", InstanceStatus::Stopped);
        let mut live = s.live_uuids();
        live.sort();
        assert_eq!(live, vec!["a".to_string(), "b".to_string()]);
    }
}
