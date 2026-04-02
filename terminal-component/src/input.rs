use web_sys::KeyboardEvent;

/// Convert a browser KeyboardEvent to a terminal escape sequence string.
/// Returns None if the key should be ignored (e.g., bare modifier keys).
pub fn key_event_to_terminal_seq(event: &KeyboardEvent) -> Option<String> {
    let key = event.key();
    let ctrl = event.ctrl_key();
    let alt = event.alt_key();
    let shift = event.shift_key();

    // Ctrl+letter → ASCII control character
    if ctrl && !alt && key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_alphabetic() {
            let ctrl_code = (ch.to_ascii_uppercase() as u8) - b'@';
            return Some(String::from(ctrl_code as char));
        }
    }

    // Alt+key → ESC prefix
    if alt && !ctrl && key.len() == 1 {
        let ch = key.chars().next()?;
        return Some(format!("\x1b{}", ch));
    }

    // Special keys
    match key.as_str() {
        "Enter" => Some("\r".into()),
        "Backspace" => {
            if ctrl {
                Some("\x17".into()) // Ctrl+Backspace = delete word
            } else {
                Some("\x7f".into())
            }
        }
        "Tab" => {
            if shift {
                Some("\x1b[Z".into()) // Shift+Tab = backtab
            } else {
                Some("\t".into())
            }
        }
        "Escape" => Some("\x1b".into()),
        "ArrowUp" => Some(arrow_key('A', ctrl, shift)),
        "ArrowDown" => Some(arrow_key('B', ctrl, shift)),
        "ArrowRight" => Some(arrow_key('C', ctrl, shift)),
        "ArrowLeft" => Some(arrow_key('D', ctrl, shift)),
        "Home" => Some(if ctrl { "\x1b[1;5H" } else { "\x1b[H" }.into()),
        "End" => Some(if ctrl { "\x1b[1;5F" } else { "\x1b[F" }.into()),
        "PageUp" => Some("\x1b[5~".into()),
        "PageDown" => Some("\x1b[6~".into()),
        "Insert" => Some("\x1b[2~".into()),
        "Delete" => Some("\x1b[3~".into()),
        "F1" => Some("\x1bOP".into()),
        "F2" => Some("\x1bOQ".into()),
        "F3" => Some("\x1bOR".into()),
        "F4" => Some("\x1bOS".into()),
        "F5" => Some("\x1b[15~".into()),
        "F6" => Some("\x1b[17~".into()),
        "F7" => Some("\x1b[18~".into()),
        "F8" => Some("\x1b[19~".into()),
        "F9" => Some("\x1b[20~".into()),
        "F10" => Some("\x1b[21~".into()),
        "F11" => Some("\x1b[23~".into()),
        "F12" => Some("\x1b[24~".into()),
        // Modifier-only keys: ignore
        "Shift" | "Control" | "Alt" | "Meta" | "CapsLock" | "NumLock" => None,
        // Regular printable character
        other if other.len() == 1 => Some(other.into()),
        // Composed characters (emoji, etc.)
        other if !other.is_empty() && !other.starts_with("Dead") => Some(other.into()),
        _ => None,
    }
}

fn arrow_key(dir: char, ctrl: bool, shift: bool) -> String {
    let modifier = match (ctrl, shift) {
        (true, true) => 6,
        (true, false) => 5,
        (false, true) => 2,
        (false, false) => 0,
    };
    if modifier > 0 {
        format!("\x1b[1;{}{}", modifier, dir)
    } else {
        format!("\x1b[{}", dir)
    }
}
