use chrono::{Duration as ChronoDuration, Utc};
use echoflow_core::domain::{
    AppPreferences, DeliveryMode, DictationError, HistoryEntry, SettingsExportPayload,
    MAX_VOCABULARY_TERMS, SETTINGS_EXPORT_SCHEMA_VERSION,
};
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use uuid::Uuid;

#[tokio::test]
async fn history_entry_persists_lists_chronologically_searches_and_deletes() {
    let store = SqliteLocalStore::open_in_memory().unwrap();

    let older = HistoryEntry {
        id: "h1".to_string(),
        final_transcript: "First dictation about Rust".to_string(),
        created_at_utc: "2026-11-01T10:00:00Z".to_string(),
        duration_ms: 1200,
        was_partial: false,
        delivery_mode: DeliveryMode::TextInsertion,
    };
    let newer = HistoryEntry {
        id: "h2".to_string(),
        final_transcript: "Second dictation about EchoFlow".to_string(),
        created_at_utc: "2026-11-02T10:00:00Z".to_string(),
        duration_ms: 2500,
        was_partial: true,
        delivery_mode: DeliveryMode::FallbackDelivery,
    };

    store.save_history_entry(&older).await.unwrap();
    store.save_history_entry(&newer).await.unwrap();

    let listed = store.list_history_entries().await.unwrap();
    assert_eq!(listed.len(), 2);
    // Newest first (chronological descending)
    assert_eq!(listed[0].id, "h2");
    assert_eq!(listed[0].was_partial, true);
    assert_eq!(listed[0].delivery_mode, DeliveryMode::FallbackDelivery);
    assert_eq!(listed[1].id, "h1");

    // Case-insensitive search
    let found = store.search_history_entries("echoflow").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "h2");

    // Individual delete
    assert!(store.delete_history_entry("h1").await.unwrap());
    assert!(!store.delete_history_entry("nonexistent").await.unwrap());
    assert_eq!(store.list_history_entries().await.unwrap().len(), 1);

    // Clear all
    let cleared = store.clear_history().await.unwrap();
    assert_eq!(cleared, 1);
    assert!(store.list_history_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn retention_pruning_runs_on_startup_and_when_retention_days_change() {
    let db_path = std::env::temp_dir().join(format!("echoflow-test-{}.db", Uuid::new_v4()));

    // Seed DB with an entry 40 days old, one 10 days old, and one 1 day old.
    {
        let store = SqliteLocalStore::open(&db_path).unwrap();
        let now = Utc::now();
        let entries = [
            ("old-40", now - ChronoDuration::days(40)),
            ("mid-10", now - ChronoDuration::days(10)),
            ("new-1", now - ChronoDuration::days(1)),
        ];
        for (id, ts) in entries {
            store
                .save_history_entry(&HistoryEntry {
                    id: id.to_string(),
                    final_transcript: format!("transcript {id}"),
                    created_at_utc: ts.to_rfc3339(),
                    duration_ms: 500,
                    was_partial: false,
                    delivery_mode: DeliveryMode::TextInsertion,
                })
                .await
                .unwrap();
        }
    }

    // Reopen: default retention is 30 days, so old-40 is pruned on startup.
    let store = SqliteLocalStore::open(&db_path).unwrap();
    let after_startup = store.list_history_entries().await.unwrap();
    assert_eq!(after_startup.len(), 2);
    assert!(after_startup.iter().all(|e| e.id != "old-40"));

    // Change retention days to 5: mid-10 is pruned immediately.
    let mut prefs = store.get_preferences().await.unwrap();
    assert_eq!(prefs.history_retention_days, 30);
    prefs.history_retention_days = 5;
    store.save_preferences(&prefs).await.unwrap();

    let after_update = store.list_history_entries().await.unwrap();
    assert_eq!(after_update.len(), 1);
    assert_eq!(after_update[0].id, "new-1");

    drop(store);
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn non_positive_or_out_of_range_retention_days_are_rejected() {
    let store = SqliteLocalStore::open_in_memory().unwrap();

    for invalid_days in [0, 3651, u32::MAX] {
        let prefs = AppPreferences {
            history_retention_days: invalid_days,
            ..AppPreferences::default()
        };
        let err = store.save_preferences(&prefs).await.unwrap_err();
        assert!(
            matches!(err, DictationError::InvalidInput(_)),
            "expected InvalidInput for retention_days={invalid_days}, got {err:?}"
        );
    }

    // Preferences remain at default 30 days.
    assert_eq!(
        store
            .get_preferences()
            .await
            .unwrap()
            .history_retention_days,
        30
    );
}

#[tokio::test]
async fn custom_vocabulary_crud_search_and_negative_boundaries() {
    let store = SqliteLocalStore::open_in_memory().unwrap();

    // Empty / whitespace rejected
    for blank in ["", "   ", "\t\n"] {
        let err = store.add_vocabulary_term(blank).await.unwrap_err();
        assert!(matches!(err, DictationError::InvalidInput(_)));
    }

    // Add & trim
    let added = store.add_vocabulary_term("  EchoFlow  ").await.unwrap();
    assert_eq!(added.term, "EchoFlow");
    let second = store.add_vocabulary_term("UniFFI").await.unwrap();

    // Duplicate (exact and case-insensitive) rejected
    let dup1 = store.add_vocabulary_term("EchoFlow").await.unwrap_err();
    assert!(matches!(dup1, DictationError::InvalidInput(_)));
    let dup2 = store.add_vocabulary_term("echoflow").await.unwrap_err();
    assert!(matches!(dup2, DictationError::InvalidInput(_)));

    // Search
    let found = store.search_vocabulary_entries("uni").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, second.id);

    // Edit: empty rejected, duplicate with other entry rejected, valid edit + casing update allowed
    assert!(matches!(
        store.update_vocabulary_term(&added.id, "  ").await,
        Err(DictationError::InvalidInput(_))
    ));
    assert!(matches!(
        store.update_vocabulary_term(&added.id, "uniffi").await,
        Err(DictationError::InvalidInput(_))
    ));
    let updated = store
        .update_vocabulary_term(&added.id, "echoflow-core")
        .await
        .unwrap();
    assert_eq!(updated.term, "echoflow-core");

    // Delete
    assert!(store.delete_vocabulary_term(&second.id).await.unwrap());
    assert!(!store.delete_vocabulary_term("missing").await.unwrap());
    assert_eq!(
        store.list_vocabulary_terms().await.unwrap(),
        vec!["echoflow-core"]
    );
}

#[tokio::test]
async fn custom_vocabulary_enforces_1000_term_cap() {
    let store = SqliteLocalStore::open_in_memory().unwrap();

    for i in 0..MAX_VOCABULARY_TERMS {
        store
            .add_vocabulary_term(&format!("term-{i:04}"))
            .await
            .unwrap();
    }
    assert_eq!(
        store.list_vocabulary_entries().await.unwrap().len(),
        MAX_VOCABULARY_TERMS
    );

    // 1001st term must be rejected
    let err = store
        .add_vocabulary_term("term-overflow")
        .await
        .unwrap_err();
    assert!(matches!(err, DictationError::InvalidInput(_)));
    assert_eq!(
        store.list_vocabulary_entries().await.unwrap().len(),
        MAX_VOCABULARY_TERMS
    );
}

#[tokio::test]
async fn json_export_includes_preferences_and_vocabulary_and_excludes_history() {
    let store = SqliteLocalStore::open_in_memory().unwrap();
    store
        .save_preferences(&AppPreferences {
            push_to_talk_hotkey: "Ctrl+Shift+Space".to_string(),
            microphone_id: Some("usb-mic-1".to_string()),
            restore_clipboard: false,
            clipboard_restore_delay_ms: 200,
            history_retention_days: 14,
        })
        .await
        .unwrap();
    store.add_vocabulary_term("EchoFlow").await.unwrap();
    store.add_vocabulary_term("Tauri").await.unwrap();
    store
        .save_history_entry(&HistoryEntry {
            id: "secret-hist-1".to_string(),
            final_transcript: "Private transcript should never be exported".to_string(),
            created_at_utc: Utc::now().to_rfc3339(),
            duration_ms: 900,
            was_partial: false,
            delivery_mode: DeliveryMode::TextInsertion,
        })
        .await
        .unwrap();

    let json = store.export_settings_json().await.unwrap();
    assert!(!json.contains("Private transcript should never be exported"));
    assert!(!json.contains("secret-hist-1"));
    assert!(!json.contains("api_key"));
    assert!(!json.contains("credential"));

    let parsed: SettingsExportPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.schema_version, SETTINGS_EXPORT_SCHEMA_VERSION);
    assert_eq!(parsed.preferences.push_to_talk_hotkey, "Ctrl+Shift+Space");
    assert_eq!(parsed.preferences.history_retention_days, 14);
    assert_eq!(parsed.vocabulary, vec!["EchoFlow", "Tauri"]);

    // Import into a fresh store
    let target = SqliteLocalStore::open_in_memory().unwrap();
    target.import_settings_json(&json).await.unwrap();
    assert_eq!(
        target.get_preferences().await.unwrap().push_to_talk_hotkey,
        "Ctrl+Shift+Space"
    );
    assert_eq!(
        target.list_vocabulary_terms().await.unwrap(),
        vec!["EchoFlow", "Tauri"]
    );
}

#[tokio::test]
async fn json_import_rejects_malformed_unsupported_version_and_over_cap_without_corrupting_state() {
    let store = SqliteLocalStore::open_in_memory().unwrap();
    store
        .save_preferences(&AppPreferences {
            push_to_talk_hotkey: "Alt+Space".to_string(),
            history_retention_days: 30,
            ..AppPreferences::default()
        })
        .await
        .unwrap();
    store.add_vocabulary_term("ExistingTerm").await.unwrap();

    // 1. Malformed JSON
    let err = store
        .import_settings_json("{not valid json")
        .await
        .unwrap_err();
    assert!(matches!(err, DictationError::InvalidInput(_)));

    // 2. Unsupported schema version
    let bad_version = serde_json::json!({
        "schema_version": 999,
        "exported_at_utc": "2026-11-22T00:00:00Z",
        "preferences": AppPreferences::default(),
        "vocabulary": ["NewTerm"]
    })
    .to_string();
    let err = store.import_settings_json(&bad_version).await.unwrap_err();
    assert!(matches!(err, DictationError::InvalidInput(_)));

    // 3. Exceeds 1,000 vocabulary terms
    let over_cap_vocab: Vec<String> = (0..=MAX_VOCABULARY_TERMS)
        .map(|i| format!("word-{i}"))
        .collect();
    let over_cap = serde_json::json!({
        "schema_version": SETTINGS_EXPORT_SCHEMA_VERSION,
        "exported_at_utc": "2026-11-22T00:00:00Z",
        "preferences": {
            "push_to_talk_hotkey": "Ctrl+Space",
            "microphone_id": null,
            "restore_clipboard": true,
            "clipboard_restore_delay_ms": 150,
            "history_retention_days": 10
        },
        "vocabulary": over_cap_vocab
    })
    .to_string();
    let err = store.import_settings_json(&over_cap).await.unwrap_err();
    assert!(matches!(err, DictationError::InvalidInput(_)));

    // Existing state must remain completely intact (transactional rejection)
    assert_eq!(
        store.get_preferences().await.unwrap().push_to_talk_hotkey,
        "Alt+Space"
    );
    assert_eq!(
        store.list_vocabulary_terms().await.unwrap(),
        vec!["ExistingTerm"]
    );
}
