use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::DictationError;
use crate::gemini::GeminiLiveClient;
use crate::platform::PlatformAdapter;

const LINUX_SECRET_SERVICE_GUIDANCE: &str = "OS Secret Service (libsecret) is unavailable. Start a keyring daemon such as gnome-keyring or KWallet with Secret Service enabled so your Provider Credential can be stored securely.";

/// Observable status of the Provider Credential in the OS native credential store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCredentialStatus {
    pub configured: bool,
    pub masked_preview: Option<String>,
    pub secret_service_available: bool,
    pub degraded_reason: Option<String>,
    pub guidance: Option<String>,
}

/// Result of a live Provider Credential verification handshake against `gemini-3.5-transcribe-live`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTestResult {
    pub success: bool,
    pub message: String,
    pub error_kind: Option<String>,
}

/// Core service for validating, storing, reading, deleting, and testing the Provider Credential.
///
/// Never persists secrets to SQLite/JSON or writes raw keys into logs or error strings.
pub struct ProviderCredentialService {
    platform: Arc<dyn PlatformAdapter>,
}

impl ProviderCredentialService {
    pub fn new(platform: Arc<dyn PlatformAdapter>) -> Self {
        Self { platform }
    }

    /// Validate and normalize a user-supplied Provider Credential.
    pub fn validate_api_key(raw_api_key: &str) -> Result<&str, DictationError> {
        let trimmed = raw_api_key.trim();
        if trimmed.is_empty() {
            return Err(DictationError::InvalidInput(
                "Provider Credential cannot be empty".to_string(),
            ));
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(DictationError::InvalidInput(
                "malformed Provider Credential: whitespace is not allowed".to_string(),
            ));
        }
        Ok(trimmed)
    }

    /// Produce a redacted preview of a Provider Credential for UI display.
    pub fn mask_api_key(api_key: &str) -> String {
        let chars: Vec<char> = api_key.chars().collect();
        if chars.len() >= 8 {
            let suffix: String = chars[chars.len() - 4..].iter().collect();
            format!("••••{suffix}")
        } else {
            "••••".to_string()
        }
    }

    /// Store a validated Provider Credential in the OS native keyring.
    pub async fn save(&self, raw_api_key: &str) -> Result<(), DictationError> {
        let key = Self::validate_api_key(raw_api_key)?;
        if let Err(err) = self.platform.set_provider_credential(key).await {
            let safe_cause = redact_secret(&err.to_string(), key);
            tracing::warn!(
                status = "DEGRADED",
                component = "credentials",
                cause = %safe_cause,
                "failed to store Provider Credential in OS native keyring"
            );
            return Err(redact_error(err, key));
        }
        Ok(())
    }

    /// Read the Provider Credential from the OS native keyring without logging it.
    pub async fn get(&self) -> Result<Option<String>, DictationError> {
        match self.platform.get_provider_credential().await {
            Ok(Some(key)) if key.trim().is_empty() => Ok(None),
            Ok(other) => Ok(other),
            Err(err) => {
                let safe_cause = redact_api_key_tokens(&err.to_string());
                tracing::warn!(
                    status = "DEGRADED",
                    component = "credentials",
                    cause = %safe_cause,
                    "failed to read Provider Credential from OS native keyring"
                );
                Err(redact_error(err, ""))
            }
        }
    }

    /// Delete the Provider Credential from the OS native keyring.
    pub async fn delete(&self) -> Result<(), DictationError> {
        if let Err(err) = self.platform.delete_provider_credential().await {
            tracing::warn!(
                status = "DEGRADED",
                component = "credentials",
                cause = %err,
                "failed to delete Provider Credential from OS native keyring"
            );
            return Err(err);
        }
        Ok(())
    }

    /// Inspect Provider Credential presence and OS Secret Service availability.
    pub async fn status(&self) -> ProviderCredentialStatus {
        let caps = self.platform.probe_capabilities().await.unwrap_or_default();
        match self.platform.get_provider_credential().await {
            Ok(Some(key)) if !key.trim().is_empty() => {
                let degraded = !caps.secret_service_available;
                if degraded {
                    tracing::warn!(
                        status = "DEGRADED",
                        component = "credentials",
                        cause = "OS Secret Service daemon is unavailable",
                        "Provider Credential storage is degraded"
                    );
                }
                ProviderCredentialStatus {
                    configured: true,
                    masked_preview: Some(Self::mask_api_key(key.trim())),
                    secret_service_available: caps.secret_service_available,
                    degraded_reason: if degraded {
                        Some("OS Secret Service daemon is unavailable".to_string())
                    } else {
                        None
                    },
                    guidance: if degraded {
                        Some(LINUX_SECRET_SERVICE_GUIDANCE.to_string())
                    } else {
                        None
                    },
                }
            }
            Ok(_) => {
                let degraded = !caps.secret_service_available;
                if degraded {
                    tracing::warn!(
                        status = "DEGRADED",
                        component = "credentials",
                        cause = "OS Secret Service daemon is unavailable",
                        "Provider Credential storage is degraded"
                    );
                }
                ProviderCredentialStatus {
                    configured: false,
                    masked_preview: None,
                    secret_service_available: caps.secret_service_available,
                    degraded_reason: if degraded {
                        Some("OS Secret Service daemon is unavailable".to_string())
                    } else {
                        None
                    },
                    guidance: if degraded {
                        Some(LINUX_SECRET_SERVICE_GUIDANCE.to_string())
                    } else {
                        None
                    },
                }
            }
            Err(err) => {
                let cause = redact_api_key_tokens(&err.to_string());
                tracing::warn!(
                    status = "DEGRADED",
                    component = "credentials",
                    cause = %cause,
                    "Provider Credential storage is unavailable"
                );
                ProviderCredentialStatus {
                    configured: false,
                    masked_preview: None,
                    secret_service_available: false,
                    degraded_reason: Some(cause),
                    guidance: Some(LINUX_SECRET_SERVICE_GUIDANCE.to_string()),
                }
            }
        }
    }

    /// Execute a live verification handshake against `gemini-3.5-transcribe-live`.
    ///
    /// Tests either `candidate_key` (if provided) or the stored Provider Credential.
    pub async fn test_connection(
        &self,
        gemini: &GeminiLiveClient,
        candidate_key: Option<&str>,
    ) -> ConnectionTestResult {
        let resolved_key = match candidate_key {
            Some(raw) => match Self::validate_api_key(raw) {
                Ok(valid) => valid.to_string(),
                Err(err) => {
                    return ConnectionTestResult {
                        success: false,
                        message: err.to_string(),
                        error_kind: Some("invalid_input".to_string()),
                    };
                }
            },
            None => match self.get().await {
                Ok(Some(stored)) => stored,
                Ok(None) => {
                    return ConnectionTestResult {
                        success: false,
                        message:
                            "No Provider Credential is configured. Enter your Gemini API key first."
                                .to_string(),
                        error_kind: Some("missing_credential".to_string()),
                    };
                }
                Err(err) => {
                    return ConnectionTestResult {
                        success: false,
                        message: err.to_string(),
                        error_kind: Some("credential_store_failed".to_string()),
                    };
                }
            },
        };

        match gemini.verify_credential(&resolved_key).await {
            Ok(()) => ConnectionTestResult {
                success: true,
                message: "Connected to gemini-3.5-transcribe-live successfully.".to_string(),
                error_kind: None,
            },
            Err(err) => {
                let safe_err = redact_error(err, &resolved_key);
                let (error_kind, message) = match &safe_err {
                    DictationError::MissingProviderCredential => (
                        "missing_credential",
                        "No Provider Credential is configured. Enter your Gemini API key first."
                            .to_string(),
                    ),
                    DictationError::InvalidInput(msg) => ("invalid_input", msg.clone()),
                    DictationError::UnauthorizedProviderCredential(msg) => (
                        "unauthorized",
                        format!("Gemini rejected the Provider Credential ({msg}). Verify your API key and try again."),
                    ),
                    _ => ("transport_error", safe_err.to_string()),
                };
                tracing::warn!(
                    status = "DEGRADED",
                    component = "credentials",
                    error_kind = error_kind,
                    cause = %message,
                    "Provider Credential connection test failed"
                );
                ConnectionTestResult {
                    success: false,
                    message,
                    error_kind: Some(error_kind.to_string()),
                }
            }
        }
    }
}

fn redact_api_key_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("AIza") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx..];
        let token_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(after.len());
        if token_len >= 12 {
            out.push_str("[REDACTED]");
            rest = &after[token_len..];
        } else {
            out.push_str(&after[..token_len.max(4)]);
            rest = &after[token_len.max(4)..];
        }
    }
    out.push_str(rest);
    out
}

fn redact_secret(text: &str, secret: &str) -> String {
    let without_exact = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[REDACTED]")
    };
    redact_api_key_tokens(&without_exact)
}

fn redact_error(err: DictationError, secret: &str) -> DictationError {
    match err {
        DictationError::UnauthorizedProviderCredential(msg) => {
            DictationError::UnauthorizedProviderCredential(redact_secret(&msg, secret))
        }
        DictationError::TranscriptionTransportFailed(msg) => {
            DictationError::TranscriptionTransportFailed(redact_secret(&msg, secret))
        }
        DictationError::CredentialStoreFailed(msg) => {
            DictationError::CredentialStoreFailed(redact_secret(&msg, secret))
        }
        DictationError::InvalidInput(msg) => {
            DictationError::InvalidInput(redact_secret(&msg, secret))
        }
        other => other,
    }
}
