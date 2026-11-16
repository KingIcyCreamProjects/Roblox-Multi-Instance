//! `KeyboardEvent.code` to the key the OS injects: a PC set-1 scancode on Windows (`0xE0xx`
//! = extended) and a virtual keycode (`kVK_*`) on macOS. Physical keys, not characters, so a
//! game sees the same key whatever the keyboard layout; typed text goes through `text`.

/// (code, Windows scancode, macOS keycode)
const KEYS: &[(&str, u16, u16)] = &[
    ("KeyA", 0x1E, 0x00),
    ("KeyB", 0x30, 0x0B),
    ("KeyC", 0x2E, 0x08),
    ("KeyD", 0x20, 0x02),
    ("KeyE", 0x12, 0x0E),
    ("KeyF", 0x21, 0x03),
    ("KeyG", 0x22, 0x05),
    ("KeyH", 0x23, 0x04),
    ("KeyI", 0x17, 0x22),
    ("KeyJ", 0x24, 0x26),
    ("KeyK", 0x25, 0x28),
    ("KeyL", 0x26, 0x25),
    ("KeyM", 0x32, 0x2E),
    ("KeyN", 0x31, 0x2D),
    ("KeyO", 0x18, 0x1F),
    ("KeyP", 0x19, 0x23),
    ("KeyQ", 0x10, 0x0C),
    ("KeyR", 0x13, 0x0F),
    ("KeyS", 0x1F, 0x01),
    ("KeyT", 0x14, 0x11),
    ("KeyU", 0x16, 0x20),
    ("KeyV", 0x2F, 0x09),
    ("KeyW", 0x11, 0x0D),
    ("KeyX", 0x2D, 0x07),
    ("KeyY", 0x15, 0x10),
    ("KeyZ", 0x2C, 0x06),
    ("Digit1", 0x02, 0x12),
    ("Digit2", 0x03, 0x13),
    ("Digit3", 0x04, 0x14),
    ("Digit4", 0x05, 0x15),
    ("Digit5", 0x06, 0x17),
    ("Digit6", 0x07, 0x16),
    ("Digit7", 0x08, 0x1A),
    ("Digit8", 0x09, 0x1C),
    ("Digit9", 0x0A, 0x19),
    ("Digit0", 0x0B, 0x1D),
    ("Enter", 0x1C, 0x24),
    ("Escape", 0x01, 0x35),
    ("Backspace", 0x0E, 0x33),
    ("Tab", 0x0F, 0x30),
    ("Space", 0x39, 0x31),
    ("Minus", 0x0C, 0x1B),
    ("Equal", 0x0D, 0x18),
    ("BracketLeft", 0x1A, 0x21),
    ("BracketRight", 0x1B, 0x1E),
    ("Backslash", 0x2B, 0x2A),
    ("IntlBackslash", 0x56, 0x0A),
    ("Semicolon", 0x27, 0x29),
    ("Quote", 0x28, 0x27),
    ("Backquote", 0x29, 0x32),
    ("Comma", 0x33, 0x2B),
    ("Period", 0x34, 0x2F),
    ("Slash", 0x35, 0x2C),
    ("CapsLock", 0x3A, 0x39),
    ("F1", 0x3B, 0x7A),
    ("F2", 0x3C, 0x78),
    ("F3", 0x3D, 0x63),
    ("F4", 0x3E, 0x76),
    ("F5", 0x3F, 0x60),
    ("F6", 0x40, 0x61),
    ("F7", 0x41, 0x62),
    ("F8", 0x42, 0x64),
    ("F9", 0x43, 0x65),
    ("F10", 0x44, 0x6D),
    ("F11", 0x57, 0x67),
    ("F12", 0x58, 0x6F),
    ("PrintScreen", 0xE037, 0x69),
    ("ScrollLock", 0x46, 0x6B),
    ("Insert", 0xE052, 0x72),
    ("Home", 0xE047, 0x73),
    ("PageUp", 0xE049, 0x74),
    ("Delete", 0xE053, 0x75),
    ("End", 0xE04F, 0x77),
    ("PageDown", 0xE051, 0x79),
    ("ArrowRight", 0xE04D, 0x7C),
    ("ArrowLeft", 0xE04B, 0x7B),
    ("ArrowDown", 0xE050, 0x7D),
    ("ArrowUp", 0xE048, 0x7E),
    ("NumLock", 0x45, 0x47),
    ("NumpadDivide", 0xE035, 0x4B),
    ("NumpadMultiply", 0x37, 0x43),
    ("NumpadSubtract", 0x4A, 0x4E),
    ("NumpadAdd", 0x4E, 0x45),
    ("NumpadEnter", 0xE01C, 0x4C),
    ("NumpadDecimal", 0x53, 0x41),
    ("Numpad0", 0x52, 0x52),
    ("Numpad1", 0x4F, 0x53),
    ("Numpad2", 0x50, 0x54),
    ("Numpad3", 0x51, 0x55),
    ("Numpad4", 0x4B, 0x56),
    ("Numpad5", 0x4C, 0x57),
    ("Numpad6", 0x4D, 0x58),
    ("Numpad7", 0x47, 0x59),
    ("Numpad8", 0x48, 0x5B),
    ("Numpad9", 0x49, 0x5C),
    ("ControlLeft", 0x1D, 0x3B),
    ("ShiftLeft", 0x2A, 0x38),
    ("AltLeft", 0x38, 0x3A),
    ("MetaLeft", 0xE05B, 0x37),
    ("ControlRight", 0xE01D, 0x3E),
    ("ShiftRight", 0x36, 0x3C),
    ("AltRight", 0xE038, 0x3D),
    ("MetaRight", 0xE05C, 0x36),
    ("ContextMenu", 0xE05D, 0x6E),
    // JIS
    ("IntlRo", 0x73, 0x5E),
    ("IntlYen", 0x7D, 0x5D),
    ("KanaMode", 0x70, 0x68),
];

/// Media keys and Pause (`media` cap): no plain scancode, so Windows gets the virtual key;
/// macOS sends them as system events (desk.rs). (code, Windows `VK_*`)
const MEDIA: &[(&str, u16)] = &[
    ("AudioVolumeUp", 0xAF),
    ("AudioVolumeDown", 0xAE),
    ("AudioVolumeMute", 0xAD),
    ("MediaPlayPause", 0xB3),
    ("MediaTrackNext", 0xB0),
    ("MediaTrackPrevious", 0xB1),
    ("MediaStop", 0xB2),
    ("Pause", 0x13),
];

fn find(code: &str) -> Option<&'static (&'static str, u16, u16)> {
    KEYS.iter().find(|k| k.0 == code)
}

/// A code some platform can inject (held keys are tracked only for these).
pub fn known(code: &str) -> bool {
    find(code).is_some() || win_vk(code).is_some()
}

/// Windows virtual key for a media key or Pause.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn win_vk(code: &str) -> Option<u16> {
    MEDIA.iter().find(|k| k.0 == code).map(|k| k.1)
}

/// Windows set-1 scancode; `0xE0xx` marks an extended key. Unknown code: `None`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn scancode(code: &str) -> Option<u16> {
    find(code).map(|k| k.1)
}

/// macOS virtual keycode. Unknown code: `None`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn mac_keycode(code: &str) -> Option<u16> {
    find(code).map(|k| k.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_codes() {
        assert_eq!(scancode("KeyW"), Some(0x11));
        assert_eq!(scancode("ArrowUp"), Some(0xE048));
        assert_eq!(scancode("Space"), Some(0x39));
        assert_eq!(mac_keycode("KeyA"), Some(0x00));
        assert_eq!(mac_keycode("Enter"), Some(0x24));
        assert_eq!(scancode("NotAKey"), None);
        assert_eq!(scancode("keyw"), None, "codes are case-sensitive");
        assert_eq!(win_vk("AudioVolumeUp"), Some(0xAF));
        assert_eq!(win_vk("Pause"), Some(0x13));
        for c in ["MediaPlayPause", "IntlYen", "KeyA"] {
            assert!(known(c), "{c}");
        }
        assert!(!known("NotAKey"));
    }

    #[test]
    fn no_duplicates() {
        let mut codes: Vec<_> = KEYS.iter().map(|k| k.0).collect();
        let mut win: Vec<_> = KEYS.iter().map(|k| k.1).collect();
        let mut mac: Vec<_> = KEYS.iter().map(|k| k.2).collect();
        for v in [&mut win, &mut mac] {
            v.sort_unstable();
            let n = v.len();
            v.dedup();
            assert_eq!(v.len(), n, "duplicate key value");
        }
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n);
    }
}
