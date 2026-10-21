//! Text shaping used by the rules: case conversion, word splitting and
//! whitespace collapsing. Pure `&str` in, bytes out; no rule type appears here.

/// Uppercase and lowercase map one character to several (`ß` to `SS`), so they
/// are pushed character by character instead of through a `String`.
pub(super) fn extend_upper(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars() {
        push_upper(out, ch);
    }
}

pub(super) fn extend_lower(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars() {
        push_lower(out, ch);
    }
}

pub(super) fn push_upper(out: &mut Vec<u8>, ch: char) {
    for upper in ch.to_uppercase() {
        push_char(out, upper);
    }
}

pub(super) fn push_lower(out: &mut Vec<u8>, ch: char) {
    for lower in ch.to_lowercase() {
        push_char(out, lower);
    }
}

pub(super) fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buffer = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

/// The words a case style joins: runs of alphanumerics, split where a lowercase
/// letter is followed by an uppercase one. `camelCase` is two words, `HTTPServer`
/// is one.
pub(super) fn split_words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    let mut previous: Option<char> = None;
    for (at, ch) in text.char_indices() {
        if !ch.is_alphanumeric() {
            if let Some(from) = start.take() {
                words.push(&text[from..at]);
            }
            previous = Some(ch);
            continue;
        }
        let starts_word =
            start.is_none() || (ch.is_uppercase() && previous.is_some_and(char::is_lowercase));
        if starts_word && let Some(from) = start.replace(at) {
            words.push(&text[from..at]);
        }
        previous = Some(ch);
    }
    if let Some(from) = start {
        words.push(&text[from..]);
    }
    words
}

pub(super) fn push_capitalized(out: &mut Vec<u8>, word: &str) {
    let mut chars = word.chars();
    if let Some(first) = chars.next() {
        push_upper(out, first);
        for ch in chars {
            push_lower(out, ch);
        }
    }
}

pub(super) fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}
