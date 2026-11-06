//! The interface language, Indonesian or English. One switch, set from the
//! settings, that every text on screen reads through [`tr`]; the texts sit in
//! pairs where they are used, so neither language can be left out.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Globalization::GetUserDefaultUILanguage;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang {
    Indonesian,
    English,
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set(lang: Lang) {
    ENGLISH.store(lang == Lang::English, Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// The text in the language in use, given in Indonesian and in English.
pub fn tr(indonesian: &'static str, english_text: &'static str) -> &'static str {
    if english() {
        english_text
    } else {
        indonesian
    }
}

impl Lang {
    /// The language's name in `drop.ini`.
    pub fn key(self) -> &'static str {
        match self {
            Lang::Indonesian => "id",
            Lang::English => "en",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().as_str() {
            "id" | "indonesia" | "indonesian" => Some(Lang::Indonesian),
            "en" | "english" | "inggris" => Some(Lang::English),
            _ => None,
        }
    }

    /// Windows' own display language: Indonesian when it is, else English.
    pub fn system() -> Self {
        // The primary language is the low ten bits; 0x21 is Indonesian.
        if unsafe { GetUserDefaultUILanguage() } & 0x3ff == 0x21 {
            Lang::Indonesian
        } else {
            Lang::English
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_names() {
        for lang in [Lang::Indonesian, Lang::English] {
            assert_eq!(Lang::from_key(lang.key()), Some(lang));
        }
        assert_eq!(Lang::from_key(" English "), Some(Lang::English));
        assert_eq!(Lang::from_key("jawa"), None);
    }
}
