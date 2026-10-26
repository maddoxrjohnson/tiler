//! Key chords such as `alt-shift-h`, as written in the config file.

use std::fmt;
use std::str::FromStr;

/// A set of modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const ALT: Mods = Mods(1);
    pub const CTRL: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);
    pub const WIN: Mods = Mods(8);

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Every key name accepted in a binding, in a fixed order.
pub const KEY_NAMES: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "f13",
    "f14",
    "f15",
    "f16",
    "f17",
    "f18",
    "f19",
    "f20",
    "f21",
    "f22",
    "f23",
    "f24",
    "left",
    "right",
    "up",
    "down",
    "esc",
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "minus",
    "equal",
    "semicolon",
    "comma",
    "period",
    "slash",
    "backslash",
    "quote",
    "backtick",
    "leftsquarebracket",
    "rightsquarebracket",
];

/// A non-modifier key, identified by its config name (see [`KEY_NAMES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(&'static str);

impl Key {
    pub fn name(self) -> &'static str {
        self.0
    }
}

impl FromStr for Key {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        KEY_NAMES
            .iter()
            .find(|name| **name == s)
            .map(|name| Key(name))
            .ok_or_else(|| format!("unknown key '{s}'"))
    }
}

/// A key together with the modifiers that must be held, e.g. `alt-shift-h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyChord {
    pub mods: Mods,
    pub key: Key,
}

const MOD_NAMES: [(&str, Mods); 4] = [
    ("alt", Mods::ALT),
    ("ctrl", Mods::CTRL),
    ("shift", Mods::SHIFT),
    ("win", Mods::WIN),
];

impl FromStr for KeyChord {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (mod_names, key) = match s.rsplit_once('-') {
            Some((mods, key)) => (mods.split('-').collect(), key),
            None => (Vec::new(), s),
        };
        let mut mods = Mods::NONE;
        for name in mod_names {
            let (_, m) = MOD_NAMES.iter().find(|(n, _)| *n == name).ok_or_else(|| {
                format!("unknown modifier '{name}' (expected alt, ctrl, shift or win)")
            })?;
            if mods.contains(*m) {
                return Err(format!("modifier '{name}' given twice"));
            }
            mods = mods.union(*m);
        }
        Ok(KeyChord {
            mods,
            key: key.parse()?,
        })
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, m) in MOD_NAMES {
            if self.mods.contains(m) {
                write!(f, "{name}-")?;
            }
        }
        f.write_str(self.key.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chords() {
        let c: KeyChord = "alt-shift-h".parse().unwrap();
        assert_eq!(c.mods, Mods::ALT.union(Mods::SHIFT));
        assert_eq!(c.key.name(), "h");
        let c: KeyChord = "esc".parse().unwrap();
        assert!(c.mods.is_empty());
        let c: KeyChord = "ctrl-win-minus".parse().unwrap();
        assert!(c.mods.contains(Mods::CTRL) && c.mods.contains(Mods::WIN));
        assert_eq!(c.key.name(), "minus");
    }

    #[test]
    fn rejects_bad_chords() {
        assert_eq!(
            "alt-hh".parse::<KeyChord>().unwrap_err(),
            "unknown key 'hh'"
        );
        assert_eq!(
            "hyper-h".parse::<KeyChord>().unwrap_err(),
            "unknown modifier 'hyper' (expected alt, ctrl, shift or win)"
        );
        assert_eq!("alt-".parse::<KeyChord>().unwrap_err(), "unknown key ''");
        assert_eq!(
            "alt-alt-h".parse::<KeyChord>().unwrap_err(),
            "modifier 'alt' given twice"
        );
    }

    #[test]
    fn display_is_canonical() {
        let c: KeyChord = "shift-alt-1".parse().unwrap();
        assert_eq!(c.to_string(), "alt-shift-1");
    }
}
