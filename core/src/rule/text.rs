//! Text shaping used by the rules: case conversion, word splitting and
//! whitespace collapsing. Pure `&str` in, bytes out; no rule type appears here.

use unicode_normalization::char::is_combining_mark;

/// Whether `ch` belongs to a word. A combining mark is part of the letter it
/// follows, not a separator.
pub(super) fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || is_combining_mark(ch)
}

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

/// Uppercase the first character of a word. Extra characters that the
/// uppercase mapping produces are lowercased, so `ß` becomes `Ss`, not `SS`.
pub(super) fn push_title(out: &mut Vec<u8>, ch: char) {
    let mut upper = ch.to_uppercase();
    if let Some(first) = upper.next() {
        push_char(out, first);
        for extra in upper {
            push_lower(out, extra);
        }
    }
}

pub(super) fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buffer = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

/// The words a case style joins: runs of letters, digits and combining marks,
/// split where a lowercase letter is followed by an uppercase one. `camelCase`
/// is two words, `HTTPServer` is one.
pub(super) fn split_words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    let mut previous: Option<char> = None;
    for (at, ch) in text.char_indices() {
        if !is_word_char(ch) {
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
        // A mark is transparent: the next letter still sees the base letter.
        if !is_combining_mark(ch) {
            previous = Some(ch);
        }
    }
    if let Some(from) = start {
        words.push(&text[from..]);
    }
    words
}

pub(super) fn push_capitalized(out: &mut Vec<u8>, word: &str) {
    let mut chars = word.chars();
    if let Some(first) = chars.next() {
        push_title(out, first);
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
