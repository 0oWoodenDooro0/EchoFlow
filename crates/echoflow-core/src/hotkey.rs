use std::fmt;

use crate::domain::DictationError;

/// Canonical modifier keys for a Push-to-Talk Hotkey binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HotkeyModifier {
    Ctrl,
    Alt,
    Shift,
    Meta,
}

impl HotkeyModifier {
    fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(Self::Ctrl),
            "alt" | "option" | "opt" => Some(Self::Alt),
            "shift" => Some(Self::Shift),
            "meta" | "cmd" | "command" | "super" | "win" => Some(Self::Meta),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Shift => "Shift",
            Self::Meta => "Meta",
        }
    }
}

/// Validated, normalized Push-to-Talk Hotkey binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub modifiers: Vec<HotkeyModifier>,
    pub key: String,
}

impl fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for modifier in &self.modifiers {
            write!(f, "{}+", modifier.as_str())?;
        }
        write!(f, "{}", self.key)
    }
}

fn normalize_primary_key(token: &str) -> Option<&'static str> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "a" => Some("A"),
        "b" => Some("B"),
        "c" => Some("C"),
        "d" => Some("D"),
        "e" => Some("E"),
        "f" => Some("F"),
        "g" => Some("G"),
        "h" => Some("H"),
        "i" => Some("I"),
        "j" => Some("J"),
        "k" => Some("K"),
        "l" => Some("L"),
        "m" => Some("M"),
        "n" => Some("N"),
        "o" => Some("O"),
        "p" => Some("P"),
        "q" => Some("Q"),
        "r" => Some("R"),
        "s" => Some("S"),
        "t" => Some("T"),
        "u" => Some("U"),
        "v" => Some("V"),
        "w" => Some("W"),
        "x" => Some("X"),
        "y" => Some("Y"),
        "z" => Some("Z"),
        "0" => Some("0"),
        "1" => Some("1"),
        "2" => Some("2"),
        "3" => Some("3"),
        "4" => Some("4"),
        "5" => Some("5"),
        "6" => Some("6"),
        "7" => Some("7"),
        "8" => Some("8"),
        "9" => Some("9"),
        "f1" => Some("F1"),
        "f2" => Some("F2"),
        "f3" => Some("F3"),
        "f4" => Some("F4"),
        "f5" => Some("F5"),
        "f6" => Some("F6"),
        "f7" => Some("F7"),
        "f8" => Some("F8"),
        "f9" => Some("F9"),
        "f10" => Some("F10"),
        "f11" => Some("F11"),
        "f12" => Some("F12"),
        "f13" => Some("F13"),
        "f14" => Some("F14"),
        "f15" => Some("F15"),
        "f16" => Some("F16"),
        "f17" => Some("F17"),
        "f18" => Some("F18"),
        "f19" => Some("F19"),
        "f20" => Some("F20"),
        "f21" => Some("F21"),
        "f22" => Some("F22"),
        "f23" => Some("F23"),
        "f24" => Some("F24"),
        "space" | " " => Some("Space"),
        "enter" | "return" => Some("Enter"),
        "tab" => Some("Tab"),
        "escape" | "esc" => Some("Escape"),
        "backspace" => Some("Backspace"),
        "delete" | "del" => Some("Delete"),
        "insert" | "ins" => Some("Insert"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" | "pgup" => Some("PageUp"),
        "pagedown" | "pgdn" => Some("PageDown"),
        "up" | "arrowup" => Some("Up"),
        "down" | "arrowdown" => Some("Down"),
        "left" | "arrowleft" => Some("Left"),
        "right" | "arrowright" => Some("Right"),
        "capslock" => Some("CapsLock"),
        "grave" | "backquote" | "`" => Some("Backquote"),
        "minus" | "-" => Some("Minus"),
        "equal" | "=" => Some("Equal"),
        "bracketleft" | "[" => Some("BracketLeft"),
        "bracketright" | "]" => Some("BracketRight"),
        "backslash" | "\\" => Some("Backslash"),
        "semicolon" | ";" => Some("Semicolon"),
        "quote" | "'" => Some("Quote"),
        "comma" | "," => Some("Comma"),
        "period" | "." => Some("Period"),
        "slash" | "/" => Some("Slash"),
        _ => None,
    }
}

fn is_reserved_binding(canonical: &str, key: &str) -> bool {
    if key == "Escape" {
        return true;
    }
    matches!(
        canonical,
        "Ctrl+C"
            | "Ctrl+V"
            | "Ctrl+X"
            | "Ctrl+A"
            | "Ctrl+Z"
            | "Ctrl+Q"
            | "Ctrl+W"
            | "Ctrl+Alt+Delete"
            | "Meta+C"
            | "Meta+V"
            | "Meta+X"
            | "Meta+A"
            | "Meta+Z"
            | "Meta+Q"
            | "Meta+W"
            | "Meta+Tab"
            | "Meta+Space"
            | "Alt+F4"
            | "Alt+Tab"
    )
}

/// Parse, validate, and canonicalize a Push-to-Talk Hotkey binding string.
pub fn parse_hotkey(binding: &str) -> Result<HotkeyBinding, DictationError> {
    let trimmed = binding.trim();
    if trimmed.is_empty() {
        return Err(DictationError::InvalidInput(
            "Push-to-Talk Hotkey cannot be empty".to_string(),
        ));
    }

    let mut modifiers = Vec::new();
    let mut primary_key: Option<String> = None;

    for raw_part in trimmed.split('+') {
        let part = raw_part.trim();
        if part.is_empty() {
            return Err(DictationError::InvalidInput(format!(
                "invalid Push-to-Talk Hotkey `{binding}`: empty key token"
            )));
        }
        if let Some(modifier) = HotkeyModifier::from_token(part) {
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        } else if let Some(key) = normalize_primary_key(part) {
            if primary_key.is_some() {
                return Err(DictationError::InvalidInput(format!(
                    "invalid Push-to-Talk Hotkey `{binding}`: multiple primary keys"
                )));
            }
            primary_key = Some(key.to_string());
        } else {
            return Err(DictationError::InvalidInput(format!(
                "invalid Push-to-Talk Hotkey `{binding}`: unrecognized key `{part}`"
            )));
        }
    }

    let key = primary_key.ok_or_else(|| {
        DictationError::InvalidInput(format!(
            "invalid Push-to-Talk Hotkey `{binding}`: must include a primary key"
        ))
    })?;

    modifiers.sort();

    let parsed = HotkeyBinding {
        modifiers,
        key: key.clone(),
    };
    let canonical = parsed.to_string();

    if is_reserved_binding(&canonical, &key) {
        return Err(DictationError::InvalidInput(format!(
            "Push-to-Talk Hotkey `{canonical}` is reserved by the OS or application"
        )));
    }

    Ok(parsed)
}

/// Validate and normalize a Push-to-Talk Hotkey binding string into canonical form.
pub fn validate_and_normalize_hotkey(binding: &str) -> Result<String, DictationError> {
    parse_hotkey(binding).map(|parsed| parsed.to_string())
}
