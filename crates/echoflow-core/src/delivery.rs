use std::sync::Arc;
use std::time::Duration;

use crate::domain::{AppPreferences, DeliveryMode, DictationError};
use crate::platform::PlatformAdapter;

/// Executes Text Insertion with Clipboard Preservation, degrading to Fallback Delivery on failure.
pub struct DeliveryService {
    platform: Arc<dyn PlatformAdapter>,
}

/// Outcome returned by `DeliveryService::deliver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub mode: DeliveryMode,
    pub fallback_reason: Option<String>,
}

impl DeliveryService {
    pub fn new(platform: Arc<dyn PlatformAdapter>) -> Self {
        Self { platform }
    }

    /// Deliver `final_transcript` via Clipboard Preservation or Fallback Delivery.
    pub async fn deliver(
        &self,
        final_transcript: &str,
        prefs: &AppPreferences,
    ) -> Result<DeliveryOutcome, DictationError> {
        let snapshot = self.platform.read_clipboard().await?;
        self.platform.write_clipboard(final_transcript).await?;

        match self.platform.trigger_paste().await {
            Ok(()) => {
                if prefs.restore_clipboard {
                    if prefs.clipboard_restore_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(prefs.clipboard_restore_delay_ms))
                            .await;
                    }
                    if let Some(previous) = snapshot.text {
                        if let Err(err) = self.platform.write_clipboard(&previous).await {
                            tracing::warn!(
                                status = "DEGRADED",
                                component = "delivery",
                                cause = %err,
                                "failed to restore previous clipboard after Text Insertion"
                            );
                        }
                    }
                }
                Ok(DeliveryOutcome {
                    mode: DeliveryMode::TextInsertion,
                    fallback_reason: None,
                })
            }
            Err(err) => {
                let reason = err.to_string();
                tracing::warn!(
                    status = "DEGRADED",
                    component = "delivery",
                    cause = %reason,
                    "Text Insertion failed; degrading to Fallback Delivery (clipboard preserved)"
                );
                Ok(DeliveryOutcome {
                    mode: DeliveryMode::FallbackDelivery,
                    fallback_reason: Some(reason),
                })
            }
        }
    }
}
