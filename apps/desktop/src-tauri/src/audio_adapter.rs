use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use echoflow_core::audio::{f32_samples_to_i16, u16_samples_to_i16};
use echoflow_core::domain::{AudioDeviceInfo, DictationError};
use echoflow_core::platform::PcmFrame;
use tokio::sync::mpsc;

/// Enumerate available OS audio input devices via `cpal`.
pub fn list_input_devices() -> Result<Vec<AudioDeviceInfo>, DictationError> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|dev| dev.name().ok());

    let devices = host.input_devices().map_err(|e| {
        DictationError::AudioCaptureFailed(format!("failed to enumerate input devices: {e}"))
    })?;

    let mut result = Vec::new();
    for device in devices {
        let name = device
            .name()
            .unwrap_or_else(|_| "Unknown Microphone".to_string());
        let is_default = default_name.as_deref() == Some(name.as_str());
        result.push(AudioDeviceInfo {
            id: name.clone(),
            name,
            is_default,
        });
    }

    Ok(result)
}

/// Start capturing raw hardware PCM frames from the selected (or OS default) microphone via `cpal`.
///
/// Runs the non-`Send` `cpal::Stream` on a dedicated OS thread and forwards `PcmFrame`s over an
/// `mpsc::Receiver`. Dropping the receiver or encountering a hardware stream error terminates the
/// capture thread and closes the channel.
pub fn start_capture(device_id: Option<&str>) -> Result<mpsc::Receiver<PcmFrame>, DictationError> {
    let requested_id = device_id.map(str::to_string);
    let (tx, rx) = mpsc::channel::<PcmFrame>(64);
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), DictationError>>(1);

    std::thread::Builder::new()
        .name("echoflow-cpal-capture".to_string())
        .spawn(move || {
            let host = cpal::default_host();
            let device = match requested_id.as_deref() {
                Some(target_id) => {
                    let found = host.input_devices().ok().and_then(|mut devs| {
                        devs.find(|d| d.name().ok().as_deref() == Some(target_id))
                    });
                    match found {
                        Some(dev) => dev,
                        None => {
                            let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(format!(
                                "microphone '{target_id}' is not connected"
                            ))));
                            return;
                        }
                    }
                }
                None => match host.default_input_device() {
                    Some(dev) => dev,
                    None => {
                        let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(
                            "no default microphone input device available".to_string(),
                        )));
                        return;
                    }
                },
            };

            let supported_config = match device.default_input_config() {
                Ok(cfg) => cfg,
                Err(e) => {
                    let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(format!(
                        "failed to query input config: {e}"
                    ))));
                    return;
                }
            };

            let sample_format = supported_config.sample_format();
            let config: StreamConfig = supported_config.into();
            let sample_rate_hz = config.sample_rate.0;
            let channels = config.channels;

            let stream_failed = Arc::new(AtomicBool::new(false));
            let err_flag = Arc::clone(&stream_failed);
            let err_fn = move |err: cpal::StreamError| {
                tracing::warn!(
                    status = "DEGRADED",
                    component = "audio_adapter",
                    cause = %err,
                    "microphone input stream error; terminating capture"
                );
                err_flag.store(true, Ordering::Release);
            };

            let tx_cb = tx.clone();
            let stream_result = match sample_format {
                SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        let _ = tx_cb.try_send(PcmFrame {
                            samples: f32_samples_to_i16(data),
                            sample_rate_hz,
                            channels,
                        });
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        let _ = tx_cb.try_send(PcmFrame {
                            samples: data.to_vec(),
                            sample_rate_hz,
                            channels,
                        });
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::U16 => device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        let _ = tx_cb.try_send(PcmFrame {
                            samples: u16_samples_to_i16(data),
                            sample_rate_hz,
                            channels,
                        });
                    },
                    err_fn,
                    None,
                ),
                other => {
                    let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(format!(
                        "unsupported sample format: {other:?}"
                    ))));
                    return;
                }
            };

            let stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(format!(
                        "failed to build audio input stream: {e}"
                    ))));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = init_tx.send(Err(DictationError::AudioCaptureFailed(format!(
                    "failed to start audio input stream: {e}"
                ))));
                return;
            }

            let _ = init_tx.send(Ok(()));

            while !tx.is_closed() && !stream_failed.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(25));
            }
            drop(stream);
        })
        .map_err(|e| {
            DictationError::AudioCaptureFailed(format!("failed to spawn audio thread: {e}"))
        })?;

    init_rx.recv().map_err(|_| {
        DictationError::AudioCaptureFailed("audio capture thread exited during init".to_string())
    })??;

    Ok(rx)
}
