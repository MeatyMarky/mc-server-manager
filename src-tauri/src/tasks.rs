//! Registry of long-running, cancellable operations.
//!
//! Every download or install registers here and gets a `task_id` the UI can
//! cancel. The registry owns only cancellation tokens; progress travels as
//! events.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct TaskRegistry {
    tokens: Mutex<HashMap<String, CancellationToken>>,
}

impl TaskRegistry {
    pub fn register(&self) -> (String, CancellationToken) {
        let id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.insert(id.clone(), token.clone());
        }
        (id, token)
    }

    /// Returns false when the task already finished, so the UI can say so
    /// instead of pretending it cancelled something.
    pub fn cancel(&self, task_id: &str) -> bool {
        let Ok(tokens) = self.tokens.lock() else {
            return false;
        };
        match tokens.get(task_id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn finish(&self, task_id: &str) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(task_id);
        }
    }

    pub fn active(&self) -> usize {
        self.tokens.lock().map(|t| t.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_tasks_can_be_cancelled_once() {
        let registry = TaskRegistry::default();
        let (id, token) = registry.register();
        assert_eq!(registry.active(), 1);
        assert!(!token.is_cancelled());

        assert!(registry.cancel(&id));
        assert!(token.is_cancelled());

        registry.finish(&id);
        assert_eq!(registry.active(), 0);
        assert!(!registry.cancel(&id), "a finished task cannot be cancelled");
    }

    #[test]
    fn unknown_task_ids_report_false() {
        let registry = TaskRegistry::default();
        assert!(!registry.cancel("nope"));
    }

    #[test]
    fn tasks_are_independent() {
        let registry = TaskRegistry::default();
        let (first, first_token) = registry.register();
        let (_second, second_token) = registry.register();
        registry.cancel(&first);
        assert!(first_token.is_cancelled());
        assert!(!second_token.is_cancelled());
    }
}
