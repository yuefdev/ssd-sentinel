//! Tiny bilingual helpers shared by the Rust core so user-visible text follows the UI language.

/// Returns the English text when `en` is true, otherwise the Turkish text.
#[inline]
pub fn pick<'a>(en: bool, tr: &'a str, en_str: &'a str) -> &'a str {
    if en {
        en_str
    } else {
        tr
    }
}
