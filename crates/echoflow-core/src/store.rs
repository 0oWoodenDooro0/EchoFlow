use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::{AppPreferences, DictationError, HistoryEntry};

/// Persistence boundary for History Entry records, Custom Vocabulary terms, and non-secret preferences.
///
/// Issue #5 provides the full SQLite implementation behind this seam.
#[async_trait]
pub trait LocalStore: Send + Sync {
    async fn save_history_entry(&self, entry: &HistoryEntry) -> Result<(), DictationError>;
    async fn list_history_entries(&self) -> Result<Vec<HistoryEntry>, DictationError>;
    async fn list_vocabulary_terms(&self) -> Result<Vec<String>, DictationError>;
    async fn get_preferences(&self) -> Result<AppPreferences, DictationError>;
    async fn save_preferences(&self, prefs: &AppPreferences) -> Result<(), DictationError>;
}

/// Minimal in-memory `LocalStore` baseline for the Issue #2 tracer bullet.
#[derive(Debug, Default)]
pub struct InMemoryLocalStore {
    history: Mutex<Vec<HistoryEntry>>,
    vocabulary: Mutex<Vec<String>>,
    preferences: Mutex<AppPreferences>,
}

impl InMemoryLocalStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LocalStore for InMemoryLocalStore {
    async fn save_history_entry(&self, entry: &HistoryEntry) -> Result<(), DictationError> {
        let mut guard = self
            .history
            .lock()
            .map_err(|_| DictationError::StorageFailed("history lock poisoned".to_string()))?;
        guard.push(entry.clone());
        Ok(())
    }

    async fn list_history_entries(&self) -> Result<Vec<HistoryEntry>, DictationError> {
        let guard = self
            .history
            .lock()
            .map_err(|_| DictationError::StorageFailed("history lock poisoned".to_string()))?;
        Ok(guard.clone())
    }

    async fn list_vocabulary_terms(&self) -> Result<Vec<String>, DictationError> {
        let guard = self
            .vocabulary
            .lock()
            .map_err(|_| DictationError::StorageFailed("vocabulary lock poisoned".to_string()))?;
        Ok(guard.clone())
    }

    async fn get_preferences(&self) -> Result<AppPreferences, DictationError> {
        let guard = self
            .preferences
            .lock()
            .map_err(|_| DictationError::StorageFailed("preferences lock poisoned".to_string()))?;
        Ok(guard.clone())
    }

    async fn save_preferences(&self, prefs: &AppPreferences) -> Result<(), DictationError> {
        let mut guard = self
            .preferences
            .lock()
            .map_err(|_| DictationError::StorageFailed("preferences lock poisoned".to_string()))?;
        *guard = prefs.clone();
        Ok(())
    }
}
