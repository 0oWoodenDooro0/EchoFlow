use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::domain::{
    AppPreferences, DeliveryMode, DictationError, HistoryEntry, SettingsExportPayload,
    VocabularyEntry, MAX_HISTORY_RETENTION_DAYS, MAX_VOCABULARY_TERMS, MIN_HISTORY_RETENTION_DAYS,
    SETTINGS_EXPORT_SCHEMA_VERSION,
};

/// Persistence boundary for History Entry records, Custom Vocabulary terms, and non-secret preferences.
#[async_trait]
pub trait LocalStore: Send + Sync {
    async fn save_history_entry(&self, entry: &HistoryEntry) -> Result<(), DictationError>;
    async fn list_history_entries(&self) -> Result<Vec<HistoryEntry>, DictationError>;
    async fn search_history_entries(
        &self,
        query: &str,
    ) -> Result<Vec<HistoryEntry>, DictationError>;
    async fn delete_history_entry(&self, id: &str) -> Result<bool, DictationError>;
    async fn clear_history(&self) -> Result<usize, DictationError>;
    async fn list_vocabulary_entries(&self) -> Result<Vec<VocabularyEntry>, DictationError>;
    async fn search_vocabulary_entries(
        &self,
        query: &str,
    ) -> Result<Vec<VocabularyEntry>, DictationError>;
    async fn add_vocabulary_term(&self, term: &str) -> Result<VocabularyEntry, DictationError>;
    async fn update_vocabulary_term(
        &self,
        id: &str,
        new_term: &str,
    ) -> Result<VocabularyEntry, DictationError>;
    async fn delete_vocabulary_term(&self, id: &str) -> Result<bool, DictationError>;
    async fn list_vocabulary_terms(&self) -> Result<Vec<String>, DictationError>;
    async fn get_preferences(&self) -> Result<AppPreferences, DictationError>;
    async fn save_preferences(&self, prefs: &AppPreferences) -> Result<(), DictationError>;
    async fn export_settings_json(&self) -> Result<String, DictationError>;
    async fn import_settings_json(&self, json_str: &str) -> Result<(), DictationError>;
}

/// SQLite-backed `LocalStore` for History Entry, Custom Vocabulary, preferences, and JSON export/import.
pub struct SqliteLocalStore {
    conn: Mutex<Connection>,
}

impl SqliteLocalStore {
    /// Open or create a file-backed SQLite store at `path` and run startup retention pruning.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DictationError> {
        let conn = Connection::open(path)
            .map_err(|e| DictationError::StorageFailed(format!("failed to open sqlite db: {e}")))?;
        Self::init_with_connection(conn)
    }

    /// Open an in-memory SQLite store (used for automated tests) and run startup initialization.
    pub fn open_in_memory() -> Result<Self, DictationError> {
        let conn = Connection::open_in_memory().map_err(|e| {
            DictationError::StorageFailed(format!("failed to open in-memory sqlite db: {e}"))
        })?;
        Self::init_with_connection(conn)
    }

    fn init_with_connection(conn: Connection) -> Result<Self, DictationError> {
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS history_entries (
                id TEXT PRIMARY KEY,
                final_transcript TEXT NOT NULL,
                created_at_utc TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                was_partial INTEGER NOT NULL,
                delivery_mode TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_created_at
                ON history_entries (created_at_utc DESC);

            CREATE TABLE IF NOT EXISTS vocabulary_entries (
                id TEXT PRIMARY KEY,
                term TEXT NOT NULL COLLATE NOCASE UNIQUE,
                created_at_utc TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS preferences (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                push_to_talk_hotkey TEXT NOT NULL,
                microphone_id TEXT,
                restore_clipboard INTEGER NOT NULL,
                clipboard_restore_delay_ms INTEGER NOT NULL,
                history_retention_days INTEGER NOT NULL
            );
            ",
        )
        .map_err(|e| DictationError::StorageFailed(format!("failed to initialize schema: {e}")))?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        // Ensure default preferences row exists and run startup retention pruning.
        let prefs = store.get_preferences_sync()?;
        store.prune_expired_history_sync(prefs.history_retention_days, Utc::now())?;
        Ok(store)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DictationError> {
        self.conn
            .lock()
            .map_err(|_| DictationError::StorageFailed("sqlite connection lock poisoned".into()))
    }

    fn get_preferences_sync(&self) -> Result<AppPreferences, DictationError> {
        let conn = self.lock_conn()?;
        let row = conn
            .query_row(
                "SELECT push_to_talk_hotkey, microphone_id, restore_clipboard,
                        clipboard_restore_delay_ms, history_retention_days
                 FROM preferences WHERE id = 1",
                [],
                |row| {
                    let restore_int: i64 = row.get(2)?;
                    let delay_ms: i64 = row.get(3)?;
                    let retention_days: i64 = row.get(4)?;
                    Ok(AppPreferences {
                        push_to_talk_hotkey: row.get(0)?,
                        microphone_id: row.get(1)?,
                        restore_clipboard: restore_int != 0,
                        clipboard_restore_delay_ms: delay_ms.max(0) as u64,
                        history_retention_days: retention_days.max(1) as u32,
                    })
                },
            )
            .optional()
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to read preferences: {e}"))
            })?;

        match row {
            Some(prefs) => Ok(prefs),
            None => {
                let defaults = AppPreferences::default();
                Self::write_preferences_conn(&conn, &defaults)?;
                Ok(defaults)
            }
        }
    }

    fn write_preferences_conn(
        conn: &Connection,
        prefs: &AppPreferences,
    ) -> Result<(), DictationError> {
        conn.execute(
            "INSERT INTO preferences (
                id, push_to_talk_hotkey, microphone_id, restore_clipboard,
                clipboard_restore_delay_ms, history_retention_days
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                push_to_talk_hotkey = excluded.push_to_talk_hotkey,
                microphone_id = excluded.microphone_id,
                restore_clipboard = excluded.restore_clipboard,
                clipboard_restore_delay_ms = excluded.clipboard_restore_delay_ms,
                history_retention_days = excluded.history_retention_days",
            params![
                prefs.push_to_talk_hotkey,
                prefs.microphone_id,
                if prefs.restore_clipboard { 1i64 } else { 0i64 },
                prefs.clipboard_restore_delay_ms as i64,
                prefs.history_retention_days as i64,
            ],
        )
        .map_err(|e| DictationError::StorageFailed(format!("failed to save preferences: {e}")))?;
        Ok(())
    }

    fn prune_expired_history_sync(
        &self,
        retention_days: u32,
        now_utc: DateTime<Utc>,
    ) -> Result<usize, DictationError> {
        let cutoff = now_utc - ChronoDuration::days(i64::from(retention_days));
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| DictationError::StorageFailed(format!("prune tx start failed: {e}")))?;

        let expired_ids: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT id, created_at_utc FROM history_entries")
                .map_err(|e| {
                    DictationError::StorageFailed(format!("prune select prepare failed: {e}"))
                })?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| DictationError::StorageFailed(format!("prune query failed: {e}")))?;

            let mut ids = Vec::new();
            for row in rows {
                let (id, ts) = row.map_err(|e| {
                    DictationError::StorageFailed(format!("prune row read failed: {e}"))
                })?;
                if let Ok(parsed) = DateTime::parse_from_rfc3339(&ts) {
                    if parsed.with_timezone(&Utc) < cutoff {
                        ids.push(id);
                    }
                }
            }
            ids
        };

        let mut deleted = 0usize;
        for id in &expired_ids {
            deleted += tx
                .execute("DELETE FROM history_entries WHERE id = ?1", params![id])
                .map_err(|e| DictationError::StorageFailed(format!("prune delete failed: {e}")))?;
        }
        tx.commit()
            .map_err(|e| DictationError::StorageFailed(format!("prune tx commit failed: {e}")))?;
        Ok(deleted)
    }
}

fn delivery_mode_to_str(mode: DeliveryMode) -> &'static str {
    match mode {
        DeliveryMode::TextInsertion => "text_insertion",
        DeliveryMode::FallbackDelivery => "fallback_delivery",
        DeliveryMode::Aborted => "aborted",
    }
}

fn str_to_delivery_mode(value: &str) -> DeliveryMode {
    match value {
        "text_insertion" => DeliveryMode::TextInsertion,
        "fallback_delivery" => DeliveryMode::FallbackDelivery,
        "aborted" => DeliveryMode::Aborted,
        _ => DeliveryMode::FallbackDelivery,
    }
}

fn map_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let duration_ms: i64 = row.get(3)?;
    let was_partial: i64 = row.get(4)?;
    let delivery_mode_str: String = row.get(5)?;
    Ok(HistoryEntry {
        id: row.get(0)?,
        final_transcript: row.get(1)?,
        created_at_utc: row.get(2)?,
        duration_ms: duration_ms.max(0) as u64,
        was_partial: was_partial != 0,
        delivery_mode: str_to_delivery_mode(&delivery_mode_str),
    })
}

#[async_trait]
impl LocalStore for SqliteLocalStore {
    async fn save_history_entry(&self, entry: &HistoryEntry) -> Result<(), DictationError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO history_entries (
                id, final_transcript, created_at_utc, duration_ms, was_partial, delivery_mode
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                final_transcript = excluded.final_transcript,
                created_at_utc = excluded.created_at_utc,
                duration_ms = excluded.duration_ms,
                was_partial = excluded.was_partial,
                delivery_mode = excluded.delivery_mode",
            params![
                entry.id,
                entry.final_transcript,
                entry.created_at_utc,
                entry.duration_ms as i64,
                if entry.was_partial { 1i64 } else { 0i64 },
                delivery_mode_to_str(entry.delivery_mode),
            ],
        )
        .map_err(|e| DictationError::StorageFailed(format!("failed to save history entry: {e}")))?;
        Ok(())
    }

    async fn list_history_entries(&self) -> Result<Vec<HistoryEntry>, DictationError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, final_transcript, created_at_utc, duration_ms, was_partial, delivery_mode
                 FROM history_entries
                 ORDER BY created_at_utc DESC, rowid DESC",
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to prepare history query: {e}"))
            })?;
        let rows = stmt
            .query_map([], map_history_row)
            .map_err(|e| DictationError::StorageFailed(format!("failed to list history: {e}")))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| {
                DictationError::StorageFailed(format!("failed to read history row: {e}"))
            })?);
        }
        Ok(entries)
    }

    async fn search_history_entries(
        &self,
        query: &str,
    ) -> Result<Vec<HistoryEntry>, DictationError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list_history_entries().await;
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, final_transcript, created_at_utc, duration_ms, was_partial, delivery_mode
                 FROM history_entries
                 WHERE lower(final_transcript) LIKE ?1
                 ORDER BY created_at_utc DESC, rowid DESC",
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to prepare history search: {e}"))
            })?;
        let rows = stmt
            .query_map(params![pattern], map_history_row)
            .map_err(|e| DictationError::StorageFailed(format!("failed to search history: {e}")))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| {
                DictationError::StorageFailed(format!("failed to read history row: {e}"))
            })?);
        }
        Ok(entries)
    }

    async fn delete_history_entry(&self, id: &str) -> Result<bool, DictationError> {
        let conn = self.lock_conn()?;
        let affected = conn
            .execute("DELETE FROM history_entries WHERE id = ?1", params![id])
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to delete history entry: {e}"))
            })?;
        Ok(affected > 0)
    }

    async fn clear_history(&self) -> Result<usize, DictationError> {
        let conn = self.lock_conn()?;
        let affected = conn
            .execute("DELETE FROM history_entries", [])
            .map_err(|e| DictationError::StorageFailed(format!("failed to clear history: {e}")))?;
        Ok(affected)
    }

    async fn list_vocabulary_entries(&self) -> Result<Vec<VocabularyEntry>, DictationError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, term, created_at_utc
                 FROM vocabulary_entries
                 ORDER BY term COLLATE NOCASE ASC",
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to prepare vocabulary list: {e}"))
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(VocabularyEntry {
                    id: row.get(0)?,
                    term: row.get(1)?,
                    created_at_utc: row.get(2)?,
                })
            })
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to list vocabulary entries: {e}"))
            })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| {
                DictationError::StorageFailed(format!("failed to read vocabulary entry: {e}"))
            })?);
        }
        Ok(entries)
    }

    async fn search_vocabulary_entries(
        &self,
        query: &str,
    ) -> Result<Vec<VocabularyEntry>, DictationError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list_vocabulary_entries().await;
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, term, created_at_utc
                 FROM vocabulary_entries
                 WHERE lower(term) LIKE ?1
                 ORDER BY term COLLATE NOCASE ASC",
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to prepare vocabulary search: {e}"))
            })?;
        let rows = stmt
            .query_map(params![pattern], |row| {
                Ok(VocabularyEntry {
                    id: row.get(0)?,
                    term: row.get(1)?,
                    created_at_utc: row.get(2)?,
                })
            })
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to search vocabulary: {e}"))
            })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| {
                DictationError::StorageFailed(format!("failed to read vocabulary entry: {e}"))
            })?);
        }
        Ok(entries)
    }

    async fn add_vocabulary_term(&self, term: &str) -> Result<VocabularyEntry, DictationError> {
        let trimmed = term.trim();
        if trimmed.is_empty() {
            return Err(DictationError::InvalidInput(
                "Custom Vocabulary term cannot be empty".to_string(),
            ));
        }

        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vocabulary_entries", [], |row| {
                row.get(0)
            })
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to count vocabulary: {e}"))
            })?;
        if count as usize >= MAX_VOCABULARY_TERMS {
            return Err(DictationError::InvalidInput(format!(
                "Custom Vocabulary limit of {MAX_VOCABULARY_TERMS} terms reached"
            )));
        }

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vocabulary_entries WHERE term = ?1 COLLATE NOCASE)",
                params![trimmed],
                |row| row.get(0),
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to check vocabulary duplicate: {e}"))
            })?;
        if exists {
            return Err(DictationError::InvalidInput(format!(
                "Custom Vocabulary term '{trimmed}' already exists"
            )));
        }

        let entry = VocabularyEntry {
            id: Uuid::new_v4().to_string(),
            term: trimmed.to_string(),
            created_at_utc: Utc::now().to_rfc3339(),
        };
        conn.execute(
            "INSERT INTO vocabulary_entries (id, term, created_at_utc) VALUES (?1, ?2, ?3)",
            params![entry.id, entry.term, entry.created_at_utc],
        )
        .map_err(|e| {
            DictationError::StorageFailed(format!("failed to insert vocabulary term: {e}"))
        })?;

        Ok(entry)
    }

    async fn update_vocabulary_term(
        &self,
        id: &str,
        new_term: &str,
    ) -> Result<VocabularyEntry, DictationError> {
        let trimmed = new_term.trim();
        if trimmed.is_empty() {
            return Err(DictationError::InvalidInput(
                "Custom Vocabulary term cannot be empty".to_string(),
            ));
        }

        let conn = self.lock_conn()?;
        let existing_created_at: Option<String> = conn
            .query_row(
                "SELECT created_at_utc FROM vocabulary_entries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to look up vocabulary entry: {e}"))
            })?;
        let Some(created_at_utc) = existing_created_at else {
            return Err(DictationError::InvalidInput(format!(
                "Custom Vocabulary entry '{id}' not found"
            )));
        };

        let conflict: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vocabulary_entries WHERE term = ?1 COLLATE NOCASE AND id != ?2)",
                params![trimmed, id],
                |row| row.get(0),
            )
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to check vocabulary duplicate: {e}"))
            })?;
        if conflict {
            return Err(DictationError::InvalidInput(format!(
                "Custom Vocabulary term '{trimmed}' already exists"
            )));
        }

        conn.execute(
            "UPDATE vocabulary_entries SET term = ?1 WHERE id = ?2",
            params![trimmed, id],
        )
        .map_err(|e| {
            DictationError::StorageFailed(format!("failed to update vocabulary term: {e}"))
        })?;

        Ok(VocabularyEntry {
            id: id.to_string(),
            term: trimmed.to_string(),
            created_at_utc,
        })
    }

    async fn delete_vocabulary_term(&self, id: &str) -> Result<bool, DictationError> {
        let conn = self.lock_conn()?;
        let affected = conn
            .execute("DELETE FROM vocabulary_entries WHERE id = ?1", params![id])
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to delete vocabulary term: {e}"))
            })?;
        Ok(affected > 0)
    }

    async fn list_vocabulary_terms(&self) -> Result<Vec<String>, DictationError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT term FROM vocabulary_entries ORDER BY term COLLATE NOCASE ASC")
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to prepare vocabulary query: {e}"))
            })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| {
                DictationError::StorageFailed(format!("failed to list vocabulary: {e}"))
            })?;
        let mut terms = Vec::new();
        for row in rows {
            terms.push(row.map_err(|e| {
                DictationError::StorageFailed(format!("failed to read vocabulary row: {e}"))
            })?);
        }
        Ok(terms)
    }

    async fn get_preferences(&self) -> Result<AppPreferences, DictationError> {
        self.get_preferences_sync()
    }

    async fn save_preferences(&self, prefs: &AppPreferences) -> Result<(), DictationError> {
        if prefs.history_retention_days < MIN_HISTORY_RETENTION_DAYS
            || prefs.history_retention_days > MAX_HISTORY_RETENTION_DAYS
        {
            return Err(DictationError::InvalidInput(format!(
                "history_retention_days must be between {MIN_HISTORY_RETENTION_DAYS} and {MAX_HISTORY_RETENTION_DAYS}, got {}",
                prefs.history_retention_days
            )));
        }
        {
            let conn = self.lock_conn()?;
            Self::write_preferences_conn(&conn, prefs)?;
        }
        self.prune_expired_history_sync(prefs.history_retention_days, Utc::now())?;
        Ok(())
    }

    async fn export_settings_json(&self) -> Result<String, DictationError> {
        let preferences = self.get_preferences_sync()?;
        let vocabulary = self.list_vocabulary_terms().await?;
        let payload = SettingsExportPayload {
            schema_version: SETTINGS_EXPORT_SCHEMA_VERSION,
            exported_at_utc: Utc::now().to_rfc3339(),
            preferences,
            vocabulary,
        };
        serde_json::to_string_pretty(&payload).map_err(|e| {
            DictationError::StorageFailed(format!("failed to serialize settings export: {e}"))
        })
    }

    async fn import_settings_json(&self, json_str: &str) -> Result<(), DictationError> {
        let payload: SettingsExportPayload = serde_json::from_str(json_str).map_err(|e| {
            DictationError::InvalidInput(format!("malformed settings import JSON: {e}"))
        })?;

        if payload.schema_version != SETTINGS_EXPORT_SCHEMA_VERSION {
            return Err(DictationError::InvalidInput(format!(
                "unsupported settings schema_version {}; expected {SETTINGS_EXPORT_SCHEMA_VERSION}",
                payload.schema_version
            )));
        }

        if payload.preferences.history_retention_days < MIN_HISTORY_RETENTION_DAYS
            || payload.preferences.history_retention_days > MAX_HISTORY_RETENTION_DAYS
        {
            return Err(DictationError::InvalidInput(format!(
                "history_retention_days must be between {MIN_HISTORY_RETENTION_DAYS} and {MAX_HISTORY_RETENTION_DAYS}, got {}",
                payload.preferences.history_retention_days
            )));
        }

        if payload.vocabulary.len() > MAX_VOCABULARY_TERMS {
            return Err(DictationError::InvalidInput(format!(
                "imported Custom Vocabulary exceeds {MAX_VOCABULARY_TERMS} term limit ({})",
                payload.vocabulary.len()
            )));
        }

        let mut normalized_vocab = Vec::with_capacity(payload.vocabulary.len());
        let mut seen = HashSet::with_capacity(payload.vocabulary.len());
        for raw_term in &payload.vocabulary {
            let trimmed = raw_term.trim();
            if trimmed.is_empty() {
                return Err(DictationError::InvalidInput(
                    "imported Custom Vocabulary contains an empty term".to_string(),
                ));
            }
            let key = trimmed.to_lowercase();
            if !seen.insert(key) {
                return Err(DictationError::InvalidInput(format!(
                    "imported Custom Vocabulary contains duplicate term '{trimmed}'"
                )));
            }
            normalized_vocab.push(trimmed.to_string());
        }

        {
            let mut conn = self.lock_conn()?;
            let tx = conn.transaction().map_err(|e| {
                DictationError::StorageFailed(format!("failed to start import transaction: {e}"))
            })?;

            Self::write_preferences_conn(&tx, &payload.preferences)?;
            tx.execute("DELETE FROM vocabulary_entries", [])
                .map_err(|e| {
                    DictationError::StorageFailed(format!("failed to clear vocabulary: {e}"))
                })?;

            let now = Utc::now().to_rfc3339();
            for term in &normalized_vocab {
                tx.execute(
                    "INSERT INTO vocabulary_entries (id, term, created_at_utc) VALUES (?1, ?2, ?3)",
                    params![Uuid::new_v4().to_string(), term, now],
                )
                .map_err(|e| {
                    DictationError::StorageFailed(format!("failed to import vocabulary term: {e}"))
                })?;
            }

            tx.commit().map_err(|e| {
                DictationError::StorageFailed(format!("failed to commit import transaction: {e}"))
            })?;
        }

        self.prune_expired_history_sync(payload.preferences.history_retention_days, Utc::now())?;
        Ok(())
    }
}
