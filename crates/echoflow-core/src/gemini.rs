use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::DictationError;

/// Default Gemini Live transcription model (ADR-0002).
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.5-transcribe-live";

/// Default transcription mode (ADR-0002).
pub const DEFAULT_GEMINI_MODE: &str = "SMART";

/// Maximum Custom Vocabulary biasing terms forwarded in session setup (ADR-0002).
pub const MAX_VOCABULARY_TERMS: usize = 1_000;

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

/// Outbound setup envelope sent immediately after WebSocket connect (`BidiGenerateContent`).
///
/// Sends a JSON `setup` frame followed by `realtimeInput.mediaChunks` JSON text frames
/// containing base64-encoded 16-bit 16kHz mono little-endian PCM audio, and a
/// `realtimeInput.audioStreamEnd` JSON text frame on release.
#[derive(Debug, Clone, Serialize)]
struct SetupEnvelope {
    setup: SetupPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupPayload {
    model: String,
    generation_config: GenerationConfig,
    input_audio_transcription: InputAudioTranscriptionConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<SystemInstruction>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    response_modalities: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InputAudioTranscriptionConfig {
    mode: String,
}

#[derive(Debug, Clone, Serialize)]
struct SystemInstruction {
    parts: Vec<TextPart>,
}

#[derive(Debug, Clone, Serialize)]
struct TextPart {
    text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealtimeInputEnvelope<T> {
    realtime_input: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaChunksPayload {
    media_chunks: [MediaChunk; 1],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaChunk {
    mime_type: &'static str,
    data: String,
}

/// Outbound end-of-audio signal sent when Push-to-Talk Hotkey is released.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioStreamEndPayload {
    audio_stream_end: bool,
}

/// Inbound server messages parsed from the Gemini Live `BidiGenerateContent` WebSocket transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiServerEvent {
    SetupComplete,
    DraftTranscript { text: String },
    FinalTranscript { text: String },
    Error { message: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawServerMessage {
    setup_complete: Option<serde_json::Value>,
    server_content: Option<RawServerContent>,
    error: Option<RawServerError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawServerContent {
    input_transcription: Option<RawTranscription>,
    model_turn: Option<RawModelTurn>,
    turn_complete: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawTranscription {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawModelTurn {
    parts: Option<Vec<RawPart>>,
}

#[derive(Debug, Deserialize)]
struct RawPart {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawServerError {
    message: Option<String>,
}

/// Parse an inbound Gemini Live `BidiGenerateContent` JSON text frame.
pub fn parse_server_event(raw: &str) -> Result<Option<GeminiServerEvent>, serde_json::Error> {
    let msg: RawServerMessage = serde_json::from_str(raw)?;

    if msg.setup_complete.is_some() {
        return Ok(Some(GeminiServerEvent::SetupComplete));
    }

    if let Some(err) = msg.error {
        return Ok(Some(GeminiServerEvent::Error {
            message: err
                .message
                .unwrap_or_else(|| "unknown Gemini server error".to_string()),
        }));
    }

    if let Some(content) = msg.server_content {
        let extracted_text = content
            .input_transcription
            .and_then(|t| t.text)
            .or_else(|| {
                let parts = content.model_turn?.parts?;
                let mut combined = String::new();
                let mut saw_text = false;
                for part in parts {
                    if let Some(text) = part.text {
                        saw_text = true;
                        combined.push_str(&text);
                    }
                }
                saw_text.then_some(combined)
            });

        return Ok(extracted_text.map(|text| {
            if content.turn_complete == Some(true) {
                GeminiServerEvent::FinalTranscript { text }
            } else {
                GeminiServerEvent::DraftTranscript { text }
            }
        }));
    }

    Err(serde::de::Error::custom(
        "missing setupComplete, serverContent, or error field",
    ))
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

    fn build_ws_request(
        &self,
        api_key: &str,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, DictationError> {
        if api_key.trim().is_empty() {
            return Err(DictationError::MissingProviderCredential);
        }
        if api_key.chars().any(char::is_whitespace) {
            return Err(DictationError::InvalidInput(
                "malformed Provider Credential: whitespace is not allowed".to_string(),
            ));
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
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(request)
    }

    fn build_setup_json(&self, vocabulary: &[String]) -> Result<String, DictationError> {
        let filtered: Vec<String> = vocabulary
            .iter()
            .map(|term| term.trim())
            .filter(|term| !term.is_empty())
            .map(ToString::to_string)
            .collect();

        let normalized: Vec<String> = if filtered.len() > MAX_VOCABULARY_TERMS {
            tracing::warn!(
                status = "DEGRADED",
                component = "gemini",
                supplied = filtered.len(),
                cap = MAX_VOCABULARY_TERMS,
                "Custom Vocabulary exceeded cap; truncating to 1,000 terms"
            );
            filtered.into_iter().take(MAX_VOCABULARY_TERMS).collect()
        } else {
            filtered
        };

        let model = if self.config.model.starts_with("models/") {
            self.config.model.clone()
        } else {
            format!("models/{}", self.config.model)
        };

        let system_instruction = if normalized.is_empty() {
            None
        } else {
            Some(SystemInstruction {
                parts: vec![TextPart {
                    text: format!("Preferred vocabulary terms: {}", normalized.join(", ")),
                }],
            })
        };

        let setup = SetupEnvelope {
            setup: SetupPayload {
                model,
                generation_config: GenerationConfig {
                    response_modalities: vec!["TEXT"],
                },
                input_audio_transcription: InputAudioTranscriptionConfig {
                    mode: self.config.mode.clone(),
                },
                system_instruction,
            },
        };
        serde_json::to_string(&setup)
            .map_err(|e| DictationError::TranscriptionTransportFailed(e.to_string()))
    }

    /// Lightweight credential verification handshake for Settings "Test Connection".
    ///
    /// Rejects missing, malformed, or unauthorized Provider Credential responses with a
    /// clear, structured `DictationError`.
    pub async fn verify_credential(&self, api_key: &str) -> Result<(), DictationError> {
        let request = self.build_ws_request(api_key)?;
        let (ws_stream, _) = tokio::time::timeout(
            self.config.flush_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| {
            DictationError::TranscriptionTransportFailed(
                "credential verification connection timed out".to_string(),
            )
        })?
        .map_err(map_ws_error)?;

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let setup_json = self.build_setup_json(&[])?;
        ws_write
            .send(Message::Text(setup_json.into()))
            .await
            .map_err(map_ws_error)?;

        let outcome = tokio::time::timeout(self.config.flush_timeout, async {
            while let Some(item) = ws_read.next().await {
                match item {
                    Ok(Message::Text(text)) => match parse_server_event(&text) {
                        Ok(Some(GeminiServerEvent::SetupComplete))
                        | Ok(Some(GeminiServerEvent::DraftTranscript { .. }))
                        | Ok(Some(GeminiServerEvent::FinalTranscript { .. })) => return Ok(()),
                        Ok(Some(GeminiServerEvent::Error { message })) => {
                            return Err(classify_server_error(message));
                        }
                        Ok(None) => {}
                        Err(err) => {
                            return Err(DictationError::TranscriptionTransportFailed(format!(
                                "malformed server handshake response: {err}"
                            )));
                        }
                    },
                    Ok(Message::Close(Some(frame))) => {
                        if frame.code == CloseCode::Normal {
                            return Ok(());
                        }
                        return Err(classify_server_error(format!(
                            "WebSocket closed ({:?}): {}",
                            frame.code, frame.reason
                        )));
                    }
                    Ok(Message::Close(None)) => return Ok(()),
                    Ok(_) => {}
                    Err(err) => return Err(map_ws_error(err)),
                }
            }
            Ok(())
        })
        .await;

        let _ = ws_write.send(Message::Close(None)).await;

        match outcome {
            Ok(result) => result,
            Err(_) => Err(DictationError::TranscriptionTransportFailed(
                "credential verification handshake timed out".to_string(),
            )),
        }
    }

    /// Connect to the configured endpoint, send the `SMART` setup frame, and return a session handle.
    pub async fn connect(
        &self,
        api_key: &str,
        vocabulary: &[String],
    ) -> Result<GeminiLiveSession, DictationError> {
        let request = self.build_ws_request(api_key)?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(map_ws_error)?;

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let setup_json = self.build_setup_json(vocabulary)?;

        ws_write
            .send(Message::Text(setup_json.into()))
            .await
            .map_err(map_ws_error)?;

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundCommand>(64);
        let (inbound_tx, inbound_rx) =
            mpsc::channel::<Result<GeminiServerEvent, DictationError>>(64);

        tokio::spawn(async move {
            while let Some(cmd) = outbound_rx.recv().await {
                match cmd {
                    OutboundCommand::AudioChunk(bytes) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let payload = RealtimeInputEnvelope {
                            realtime_input: MediaChunksPayload {
                                media_chunks: [MediaChunk {
                                    mime_type: "audio/pcm;rate=16000",
                                    data: encoded,
                                }],
                            },
                        };
                        let Ok(chunk_json) = serde_json::to_string(&payload) else {
                            break;
                        };
                        if ws_write
                            .send(Message::Text(chunk_json.into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    OutboundCommand::EndOfAudio => {
                        let payload = RealtimeInputEnvelope {
                            realtime_input: AudioStreamEndPayload {
                                audio_stream_end: true,
                            },
                        };
                        let end_json = serde_json::to_string(&payload).unwrap_or_else(|_| {
                            r#"{"realtimeInput":{"audioStreamEnd":true}}"#.to_string()
                        });
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
                    Ok(Message::Text(text)) => match parse_server_event(&text) {
                        Ok(Some(event)) => {
                            if inbound_tx.send(Ok(event)).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(
                                status = "DEGRADED",
                                component = "gemini",
                                cause = %err,
                                "ignoring unrecognized Gemini server message"
                            );
                        }
                    },
                    Ok(Message::Close(Some(frame))) if frame.code != CloseCode::Normal => {
                        let _ = inbound_tx
                            .send(Err(DictationError::TranscriptionTransportFailed(format!(
                                "WebSocket closed ({:?}): {}",
                                frame.code, frame.reason
                            ))))
                            .await;
                        break;
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

fn map_ws_error(err: tokio_tungstenite::tungstenite::Error) -> DictationError {
    if let tokio_tungstenite::tungstenite::Error::Http(resp) = &err {
        if matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return DictationError::UnauthorizedProviderCredential(format!(
                "HTTP {}",
                resp.status()
            ));
        }
    }
    DictationError::TranscriptionTransportFailed(err.to_string())
}

fn classify_server_error(message: String) -> DictationError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("api key")
        || lower.contains("invalid key")
        || lower.contains("unauthenticated")
    {
        DictationError::UnauthorizedProviderCredential(message)
    } else {
        DictationError::TranscriptionTransportFailed(message)
    }
}
