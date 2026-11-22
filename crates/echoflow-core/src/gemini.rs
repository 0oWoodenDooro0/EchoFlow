use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::DictationError;

/// Default Gemini Live transcription model (ADR-0002).
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.5-transcribe-live";

/// Default transcription mode (ADR-0002).
pub const DEFAULT_GEMINI_MODE: &str = "SMART";

/// Configuration for `GeminiLiveClient`.
#[derive(Debug, Clone)]
pub struct GeminiClientConfig {
    pub endpoint_url: String,
    pub model: String,
    pub mode: String,
    pub flush_timeout: Duration,
}

impl Default for GeminiClientConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent".to_string(),
            model: DEFAULT_GEMINI_MODEL.to_string(),
            mode: DEFAULT_GEMINI_MODE.to_string(),
            flush_timeout: Duration::from_secs(5),
        }
    }
}

/// Outbound setup payload sent immediately after WebSocket connect.
#[derive(Debug, Clone, Serialize)]
struct SetupMessage<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    model: &'a str,
    mode: &'a str,
    vocabulary: &'a [String],
}

/// Outbound end-of-audio signal sent when Push-to-Talk Hotkey is released.
#[derive(Debug, Clone, Serialize)]
struct EndOfAudioMessage<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
}

/// Inbound server messages parsed from the WebSocket transport.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GeminiServerEvent {
    DraftTranscript { text: String },
    FinalTranscript { text: String },
    Error { message: String },
}

enum OutboundCommand {
    AudioChunk(Vec<u8>),
    EndOfAudio,
    Close,
}

/// Active WebSocket transcription session handle.
pub struct GeminiLiveSession {
    outbound_tx: mpsc::Sender<OutboundCommand>,
    inbound_rx: mpsc::Receiver<Result<GeminiServerEvent, DictationError>>,
}

impl GeminiLiveSession {
    /// Split into an audio/control sender and an inbound server event receiver.
    pub fn split(
        self,
    ) -> (
        GeminiSessionSender,
        mpsc::Receiver<Result<GeminiServerEvent, DictationError>>,
    ) {
        (
            GeminiSessionSender {
                outbound_tx: self.outbound_tx,
            },
            self.inbound_rx,
        )
    }
}

/// Cloneable sender handle for streaming PCM audio chunks and end-of-audio signals.
#[derive(Clone)]
pub struct GeminiSessionSender {
    outbound_tx: mpsc::Sender<OutboundCommand>,
}

impl GeminiSessionSender {
    pub async fn send_audio_chunk(&self, pcm_le_bytes: Vec<u8>) -> Result<(), DictationError> {
        self.outbound_tx
            .send(OutboundCommand::AudioChunk(pcm_le_bytes))
            .await
            .map_err(|_| {
                DictationError::TranscriptionTransportFailed("WebSocket session closed".to_string())
            })
    }

    pub async fn send_end_of_audio(&self) -> Result<(), DictationError> {
        self.outbound_tx
            .send(OutboundCommand::EndOfAudio)
            .await
            .map_err(|_| {
                DictationError::TranscriptionTransportFailed("WebSocket session closed".to_string())
            })
    }

    pub async fn close(&self) {
        let _ = self.outbound_tx.send(OutboundCommand::Close).await;
    }
}

/// WebSocket transport client for `gemini-3.5-transcribe-live`.
#[derive(Debug, Clone)]
pub struct GeminiLiveClient {
    config: GeminiClientConfig,
}

impl GeminiLiveClient {
    pub fn new(config: GeminiClientConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &GeminiClientConfig {
        &self.config
    }

    /// Connect to the configured endpoint, send the `SMART` setup frame, and return a session handle.
    pub async fn connect(
        &self,
        api_key: &str,
        vocabulary: &[String],
    ) -> Result<GeminiLiveSession, DictationError> {
        if api_key.trim().is_empty() {
            return Err(DictationError::MissingProviderCredential);
        }

        let mut request = self
            .config
            .endpoint_url
            .as_str()
            .into_client_request()
            .map_err(|e| DictationError::TranscriptionTransportFailed(e.to_string()))?;

        let header_value = HeaderValue::from_str(api_key)
            .map_err(|e| DictationError::InvalidInput(format!("invalid API key header: {e}")))?;
        request.headers_mut().insert("x-goog-api-key", header_value);

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| DictationError::TranscriptionTransportFailed(e.to_string()))?;

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let setup = SetupMessage {
            msg_type: "setup",
            model: &self.config.model,
            mode: &self.config.mode,
            vocabulary,
        };
        let setup_json = serde_json::to_string(&setup)
            .map_err(|e| DictationError::TranscriptionTransportFailed(e.to_string()))?;

        ws_write
            .send(Message::Text(setup_json.into()))
            .await
            .map_err(|e| DictationError::TranscriptionTransportFailed(e.to_string()))?;

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundCommand>(64);
        let (inbound_tx, inbound_rx) =
            mpsc::channel::<Result<GeminiServerEvent, DictationError>>(64);

        tokio::spawn(async move {
            while let Some(cmd) = outbound_rx.recv().await {
                match cmd {
                    OutboundCommand::AudioChunk(bytes) => {
                        if ws_write.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    OutboundCommand::EndOfAudio => {
                        let end_json = serde_json::to_string(&EndOfAudioMessage {
                            msg_type: "end_of_audio",
                        })
                        .unwrap_or_else(|_| r#"{"type":"end_of_audio"}"#.to_string());
                        if ws_write.send(Message::Text(end_json.into())).await.is_err() {
                            break;
                        }
                    }
                    OutboundCommand::Close => {
                        let _ = ws_write.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Some(item) = ws_read.next().await {
                match item {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<GeminiServerEvent>(&text) {
                            Ok(event) => {
                                if inbound_tx.send(Ok(event)).await.is_err() {
                                    break;
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    status = "DEGRADED",
                                    component = "gemini",
                                    cause = %err,
                                    "ignoring unrecognized Gemini server message"
                                );
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        let _ = inbound_tx
                            .send(Err(DictationError::TranscriptionTransportFailed(
                                err.to_string(),
                            )))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(GeminiLiveSession {
            outbound_tx,
            inbound_rx,
        })
    }
}
