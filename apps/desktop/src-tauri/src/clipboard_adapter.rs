use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use echoflow_core::domain::{DictationError, DisplayServer, PlatformCapabilities};
use echoflow_core::platform::ClipboardSnapshot;

use crate::hotkey_adapter::command_exists;

/// Desktop OS clipboard & synthetic paste adapter backing `PlatformAdapter`.
#[derive(Default)]
pub struct DesktopClipboardAdapter {
    fallback_mirror: Mutex<Option<String>>,
}

impl DesktopClipboardAdapter {
    pub fn read_clipboard(
        &self,
        caps: &PlatformCapabilities,
    ) -> Result<ClipboardSnapshot, DictationError> {
        let text = match caps.display_server {
            DisplayServer::MacOs => run_stdout("pbpaste", &[]),
            DisplayServer::Windows => {
                run_stdout("powershell", &["-NoProfile", "-Command", "Get-Clipboard"])
            }
            DisplayServer::LinuxWayland => {
                if command_exists("wl-paste") {
                    run_stdout("wl-paste", &["--no-newline"])
                } else if command_exists("xclip") {
                    run_stdout("xclip", &["-o", "-selection", "clipboard"])
                } else {
                    None
                }
            }
            DisplayServer::LinuxX11 => {
                if command_exists("xclip") {
                    run_stdout("xclip", &["-o", "-selection", "clipboard"])
                } else if command_exists("xsel") {
                    run_stdout("xsel", &["-ob"])
                } else {
                    None
                }
            }
            DisplayServer::Unknown => None,
        };

        let resolved = text.or_else(|| self.fallback_mirror.lock().unwrap().clone());
        Ok(ClipboardSnapshot { text: resolved })
    }

    pub fn write_clipboard(
        &self,
        caps: &PlatformCapabilities,
        text: &str,
    ) -> Result<(), DictationError> {
        *self.fallback_mirror.lock().unwrap() = Some(text.to_string());

        let wrote = match caps.display_server {
            DisplayServer::MacOs => run_stdin("pbcopy", &[], text),
            DisplayServer::Windows => run_stdin(
                "powershell",
                &["-NoProfile", "-Command", "$input | Set-Clipboard"],
                text,
            ),
            DisplayServer::LinuxWayland => {
                if command_exists("wl-copy") {
                    run_stdin("wl-copy", &[], text)
                } else if command_exists("xclip") {
                    run_stdin("xclip", &["-selection", "clipboard"], text)
                } else {
                    false
                }
            }
            DisplayServer::LinuxX11 => {
                if command_exists("xclip") {
                    run_stdin("xclip", &["-selection", "clipboard"], text)
                } else if command_exists("xsel") {
                    run_stdin("xsel", &["-ib"], text)
                } else {
                    false
                }
            }
            DisplayServer::Unknown => true,
        };

        if !wrote {
            tracing::warn!(
                status = "DEGRADED",
                component = "clipboard_adapter",
                cause = "OS clipboard utility unavailable; using in-memory fallback mirror",
                "OS clipboard write unavailable"
            );
        }
        Ok(())
    }

    pub fn trigger_paste(&self, caps: &PlatformCapabilities) -> Result<(), DictationError> {
        if !caps.supports_text_insertion {
            return Err(DictationError::TextInsertionFailed(
                "Text Insertion unsupported in current display environment".to_string(),
            ));
        }

        let ok = match caps.display_server {
            DisplayServer::MacOs => run_status(
                "osascript",
                &[
                    "-e",
                    "tell application \"System Events\" to keystroke \"v\" using command down",
                ],
            ),
            DisplayServer::Windows => run_status(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('^v')",
                ],
            ),
            DisplayServer::LinuxX11 => {
                if command_exists("xdotool") {
                    run_status("xdotool", &["key", "--clearmodifiers", "ctrl+v"])
                } else {
                    false
                }
            }
            DisplayServer::LinuxWayland => {
                if caps.wayland_helpers.iter().any(|h| h == "wtype") {
                    run_status("wtype", &["-M", "ctrl", "v", "-m", "ctrl"])
                } else if caps.wayland_helpers.iter().any(|h| h == "ydotool") {
                    run_status("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"])
                } else {
                    false
                }
            }
            DisplayServer::Unknown => false,
        };

        if ok {
            Ok(())
        } else {
            Err(DictationError::TextInsertionFailed(format!(
                "synthetic paste failed on {:?}",
                caps.display_server
            )))
        }
    }
}

fn run_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

fn run_stdin(cmd: &str, args: &[&str], input: &str) -> bool {
    let Ok(mut child) = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(input.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn run_status(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
