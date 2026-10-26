//! Mapping Windows virtual-key codes to tiler key names.

/// The config name of a non-modifier key, for a US keyboard layout.
pub fn key_name(vk: u32) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    const FKEYS: [&str; 24] = [
        "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14",
        "f15", "f16", "f17", "f18", "f19", "f20", "f21", "f22", "f23", "f24",
    ];
    Some(match vk {
        0x41..=0x5A => LETTERS[(vk - 0x41) as usize],
        0x30..=0x39 => DIGITS[(vk - 0x30) as usize],
        0x70..=0x87 => FKEYS[(vk - 0x70) as usize],
        0x25 => "left",
        0x26 => "up",
        0x27 => "right",
        0x28 => "down",
        0x1B => "esc",
        0x0D => "enter",
        0x09 => "tab",
        0x20 => "space",
        0x08 => "backspace",
        0x2E => "delete",
        0x2D => "insert",
        0x24 => "home",
        0x23 => "end",
        0x21 => "pageup",
        0x22 => "pagedown",
        0xBA => "semicolon",
        0xBB => "equal",
        0xBC => "comma",
        0xBD => "minus",
        0xBE => "period",
        0xBF => "slash",
        0xC0 => "backtick",
        0xDB => "leftsquarebracket",
        0xDC => "backslash",
        0xDD => "rightsquarebracket",
        0xDE => "quote",
        _ => return None,
    })
}

/// Shift, Ctrl, Alt and the Windows keys, left, right or either.
pub fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiler_core::Key;

    #[test]
    fn common_keys() {
        assert_eq!(key_name(0x48), Some("h"));
        assert_eq!(key_name(0x31), Some("1"));
        assert_eq!(key_name(0x7B), Some("f12"));
        assert_eq!(key_name(0xC0), Some("backtick"));
        assert_eq!(key_name(0xBA), Some("semicolon"));
        assert_eq!(key_name(0x12), None);
    }

    #[test]
    fn every_mapped_name_is_a_valid_config_key() {
        for vk in 0..=0xFF {
            if let Some(name) = key_name(vk) {
                assert!(name.parse::<Key>().is_ok(), "vk {vk:#x} -> {name}");
            }
        }
    }

    #[test]
    fn modifiers() {
        for vk in [0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA5] {
            assert!(is_modifier(vk));
            assert_eq!(key_name(vk), None);
        }
        assert!(!is_modifier(0x48));
    }
}
