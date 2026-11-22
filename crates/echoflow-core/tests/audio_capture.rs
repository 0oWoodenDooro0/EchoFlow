use echoflow_core::audio::{
    compute_normalized_rms, pcm_i16_to_le_bytes, AudioResampler, TARGET_CHANNELS,
    TARGET_SAMPLE_RATE_HZ,
};
use echoflow_core::platform::PcmFrame;

#[test]
fn resampler_converts_48khz_stereo_to_16khz_mono_chunks() {
    // 48kHz stereo for 100ms = 4800 stereo frames = 9600 interleaved samples.
    // Expected 16kHz mono = 1600 samples = 3200 LE bytes.
    let mut samples = Vec::with_capacity(4800 * 2);
    for i in 0..4800 {
        let t = (i as f64) / 48_000.0;
        let val = ((2.0 * std::f64::consts::PI * 440.0 * t).sin() * 12_000.0) as i16;
        samples.push(val);
        samples.push(val);
    }

    let mut resampler = AudioResampler::new(320); // 20ms chunks at 16kHz
    let frame = PcmFrame {
        samples,
        sample_rate_hz: 48_000,
        channels: 2,
    };

    let mut chunks = resampler.push_frame(&frame);
    chunks.extend(resampler.flush());

    assert_eq!(TARGET_SAMPLE_RATE_HZ, 16_000);
    assert_eq!(TARGET_CHANNELS, 1);

    let total_samples: usize = chunks.iter().map(|c| c.len()).sum();
    // 4800 / 3 = 1600 samples (±1 sample for interpolation boundary)
    assert!(
        (1599..=1601).contains(&total_samples),
        "expected ~1600 mono 16kHz samples, got {total_samples}"
    );
    // All non-final chunks should be exact chunk_size (320)
    for chunk in &chunks[..chunks.len().saturating_sub(1)] {
        assert_eq!(chunk.len(), 320);
        assert_eq!(pcm_i16_to_le_bytes(chunk).len(), 640);
    }
}

#[test]
fn resampler_converts_44100hz_stereo_across_multiple_frames_without_drift() {
    // 1 second at 44.1kHz stereo, fed in 441-frame (10ms) chunks.
    let mut resampler = AudioResampler::new(320);
    let mut total_out = 0usize;

    for block in 0..100 {
        let mut samples = Vec::with_capacity(441 * 2);
        for i in 0..441 {
            let sample_idx = block * 441 + i;
            let t = (sample_idx as f64) / 44_100.0;
            let left = ((2.0 * std::f64::consts::PI * 500.0 * t).sin() * 10_000.0) as i16;
            let right = left;
            samples.push(left);
            samples.push(right);
        }
        let chunks = resampler.push_frame(&PcmFrame {
            samples,
            sample_rate_hz: 44_100,
            channels: 2,
        });
        total_out += chunks.iter().map(|c| c.len()).sum::<usize>();
    }
    let tail = resampler.flush();
    total_out += tail.iter().map(|c| c.len()).sum::<usize>();

    // 1 second at 16kHz = 16,000 samples (±2 samples)
    assert!(
        (15_998..=16_002).contains(&total_out),
        "expected ~16000 samples for 1s of 44.1kHz audio, got {total_out}"
    );
}

#[test]
fn rms_is_strictly_clamped_between_zero_and_one() {
    assert_eq!(compute_normalized_rms(&[]), 0.0);
    assert_eq!(compute_normalized_rms(&[0, 0, 0, 0]), 0.0);

    // i16::MIN is -32768, whose magnitude exceeds i16::MAX (32767); must still clamp to 1.0.
    let min_rms = compute_normalized_rms(&[i16::MIN; 64]);
    assert!(
        (0.0..=1.0).contains(&min_rms),
        "RMS must be clamped to [0.0, 1.0], got {min_rms}"
    );
    assert!((min_rms - 1.0).abs() < f32::EPSILON);

    let max_rms = compute_normalized_rms(&[i16::MAX; 64]);
    assert!((max_rms - 1.0).abs() < f32::EPSILON);

    let mid_rms = compute_normalized_rms(&[16_384, -16_384, 16_384, -16_384]);
    assert!(mid_rms > 0.45 && mid_rms < 0.55);
}

use std::sync::Arc;
use std::time::Duration;

use echoflow_core::controller::{DictationController, SessionTimersConfig};
use echoflow_core::domain::{
    AudioDeviceInfo, ControllerEvent, DictationError, SessionState, TrayStatus,
};
use echoflow_core::gemini::{GeminiClientConfig, GeminiLiveClient};
use echoflow_core::store::{LocalStore, SqliteLocalStore};
use echoflow_core::testing::FakePlatformAdapter;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

async fn start_ws_sink(final_text: &str) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let pcm_bytes = Arc::new(Mutex::new(0usize));
    let pcm_clone = Arc::clone(&pcm_bytes);
    let final_text = final_text.to_string();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws.split();
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Binary(bytes) => {
                    *pcm_clone.lock().await += bytes.len();
                }
                Message::Text(text) => {
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if parsed.get("type").and_then(|v| v.as_str()) == Some("end_of_audio") {
                        let _ = write
                            .send(Message::Text(
                                json!({ "type": "final_transcript", "text": final_text })
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    (format!("ws://{addr}"), pcm_bytes)
}

#[tokio::test]
async fn missing_selected_microphone_falls_back_to_os_default() {
    let (ws_url, pcm_bytes) = start_ws_sink("Fallback mic ok").await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_audio_devices(vec![AudioDeviceInfo {
        id: "builtin-mic".to_string(),
        name: "Built-in Mic".to_string(),
        is_default: true,
    }]);
    // 960 samples @ 48kHz stereo (10ms) -> ~160 samples @ 16kHz mono (320 bytes)
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1200; 960 * 2],
        sample_rate_hz: 48_000,
        channels: 2,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let mut prefs = store.get_preferences().await.unwrap();
    prefs.microphone_id = Some("disconnected-usb-mic".to_string());
    store.save_preferences(&prefs).await.unwrap();

    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        Arc::clone(&store) as _,
        SessionTimersConfig::default(),
    );
    let mut events = controller.subscribe();

    controller.handle_hotkey_pressed().await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Should have fallen back to OS default (`None`).
    assert_eq!(platform.recorded_requested_device_ids(), vec![None]);

    let result = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(result.as_deref(), Some("Fallback mic ok"));
    assert!(*pcm_bytes.lock().await > 0);

    let mut saw_rms = false;
    while let Ok(ev) = events.try_recv() {
        if let ControllerEvent::AudioLevelUpdated { rms } = ev {
            assert!((0.0..=1.0).contains(&rms));
            saw_rms = true;
        }
    }
    assert!(saw_rms, "expected AudioLevelUpdated event");
}

#[tokio::test]
async fn no_available_microphone_transitions_to_error_state_and_tray_error() {
    let (ws_url, _) = start_ws_sink("").await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_audio_devices(vec![]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        store,
        SessionTimersConfig::default(),
    );
    let mut events = controller.subscribe();

    let err = controller.handle_hotkey_pressed().await.unwrap_err();
    assert!(matches!(err, DictationError::AudioCaptureFailed(_)));
    assert!(matches!(
        controller.state().await,
        SessionState::Error { .. }
    ));

    let mut saw_error_event = false;
    let mut saw_tray_error = false;
    while let Ok(ev) = events.try_recv() {
        match ev {
            ControllerEvent::ErrorOccurred { .. } => saw_error_event = true,
            ControllerEvent::TrayStatusChanged {
                status: TrayStatus::Error,
            } => saw_tray_error = true,
            _ => {}
        }
    }
    assert!(saw_error_event);
    assert!(saw_tray_error);
}

#[tokio::test]
async fn mid_session_microphone_disconnect_transitions_gracefully_to_error_state() {
    let (ws_url, _) = start_ws_sink("").await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_close_audio_after_frames(true);
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1000; 320],
        sample_rate_hz: 16_000,
        channels: 1,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        store,
        SessionTimersConfig::default(),
    );
    let mut events = controller.subscribe();

    controller.handle_hotkey_pressed().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    assert!(
        matches!(controller.state().await, SessionState::Error { .. }),
        "expected Error state after mid-session mic disconnect, got {:?}",
        controller.state().await
    );

    let mut saw_tray_error = false;
    while let Ok(ev) = events.try_recv() {
        if matches!(
            ev,
            ControllerEvent::TrayStatusChanged {
                status: TrayStatus::Error
            }
        ) {
            saw_tray_error = true;
        }
    }
    assert!(saw_tray_error);
}

#[tokio::test]
async fn controller_persists_selected_microphone_and_streams_resampled_pcm() {
    let (ws_url, pcm_bytes) = start_ws_sink("USB mic").await;

    let platform = Arc::new(FakePlatformAdapter::new());
    platform.set_credential_sync(Some("test-key".to_string()));
    platform.set_audio_devices(vec![
        AudioDeviceInfo {
            id: "builtin-mic".to_string(),
            name: "Built-in Mic".to_string(),
            is_default: true,
        },
        AudioDeviceInfo {
            id: "usb-mic".to_string(),
            name: "Blue Yeti USB".to_string(),
            is_default: false,
        },
    ]);
    // 20ms at 48kHz stereo = 960 frames * 2 channels -> 320 16kHz mono samples (640 bytes)
    platform.enqueue_audio_frames(vec![PcmFrame {
        samples: vec![1500; 960 * 2],
        sample_rate_hz: 48_000,
        channels: 2,
    }]);

    let store = Arc::new(SqliteLocalStore::open_in_memory().unwrap());
    let gemini = GeminiLiveClient::new(GeminiClientConfig {
        endpoint_url: ws_url,
        ..GeminiClientConfig::default()
    });
    let controller = DictationController::new(
        Arc::clone(&platform) as _,
        gemini,
        Arc::clone(&store) as _,
        SessionTimersConfig::default(),
    );

    controller
        .set_selected_microphone(Some("usb-mic".to_string()))
        .await
        .unwrap();
    assert_eq!(
        controller.get_selected_microphone().await.unwrap(),
        Some("usb-mic".to_string())
    );

    controller.handle_hotkey_pressed().await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        platform.recorded_requested_device_ids(),
        vec![Some("usb-mic".to_string())]
    );

    let result = controller.handle_hotkey_released().await.unwrap();
    assert_eq!(result.as_deref(), Some("USB mic"));
    assert_eq!(*pcm_bytes.lock().await, 640);
}
