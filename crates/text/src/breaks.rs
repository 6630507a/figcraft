//! Where lines may break and where carets may stop: break opportunities (a small subset of UAX #14
//! with the CJK line-start and line-end rules) and grapheme clusters (UAX #29).

use unicode_segmentation::UnicodeSegmentation;

/// The break class of a char.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Breakable space: lines break after it (it hangs at the line's end).
    Space,
    /// Ideographs, kana, Hangul and full-width forms: lines may break before and after each.
    Ideo,
    /// Opening brackets and quotes: never end a line.
    Open,
    /// Closing brackets, quotes and punctuation, small kana, the prolonged sound mark and
    /// iteration marks: never start a line.
    Close,
    /// Hyphens, dashes and the slash: lines may break after them.
    Hyphen,
    /// Everything else (letters, digits, other symbols): no break inside a run of them.
    Other,
}

fn class(c: char) -> Class {
    match c {
        '(' | '[' | '{' | '\u{2018}' | '\u{201C}' | '\u{3008}' | '\u{300A}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{3014}'
        | '\u{3016}' | '\u{3018}' | '\u{301A}' | '\u{301D}' | '\u{FF08}' | '\u{FF3B}' | '\u{FF5B}' | '\u{FF62}' => Class::Open,
        ')' | ']' | '}' | ',' | '.' | ':' | ';' | '!' | '?' | '\u{2019}' | '\u{201D}' | '\u{2025}' | '\u{2026}' | '\u{203C}'
        | '\u{2047}'..='\u{2049}' | '\u{3001}' | '\u{3002}' | '\u{3005}' | '\u{3009}' | '\u{300B}' | '\u{300D}' | '\u{300F}'
        | '\u{3011}' | '\u{3015}' | '\u{3017}' | '\u{3019}' | '\u{301B}' | '\u{301C}' | '\u{301E}' | '\u{301F}' | '\u{303B}'
        | '\u{3041}' | '\u{3043}' | '\u{3045}' | '\u{3047}' | '\u{3049}' | '\u{3063}' | '\u{3083}' | '\u{3085}' | '\u{3087}'
        | '\u{308E}' | '\u{3095}' | '\u{3096}' | '\u{309D}' | '\u{309E}' | '\u{30A0}' | '\u{30A1}' | '\u{30A3}' | '\u{30A5}'
        | '\u{30A7}' | '\u{30A9}' | '\u{30C3}' | '\u{30E3}' | '\u{30E5}' | '\u{30E7}' | '\u{30EE}' | '\u{30F5}' | '\u{30F6}'
        | '\u{30FB}' | '\u{30FC}' | '\u{30FD}' | '\u{30FE}' | '\u{31F0}'..='\u{31FF}' | '\u{FF01}' | '\u{FF09}' | '\u{FF0C}'
        | '\u{FF0E}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}' | '\u{FF3D}' | '\u{FF5D}' | '\u{FF61}' | '\u{FF63}'..='\u{FF65}'
        | '\u{FF70}' => Class::Close,
        ' ' | '\t' | '\u{3000}' | '\u{2002}'..='\u{200B}' => Class::Space,
        '-' | '/' | '\u{2010}' | '\u{2013}' | '\u{2014}' => Class::Hyphen,
        '\u{2E80}'..='\u{2FFF}'
        | '\u{3003}'..='\u{33FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{A960}'..='\u{A97F}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF00}'..='\u{FFEF}'
        | '\u{20000}'..='\u{3FFFF}' => Class::Ideo,
        _ => Class::Other,
    }
}

/// Whether a line may break between `a` and the `b` after it (the break goes before `b`).
pub(crate) fn break_between(a: char, b: char) -> bool {
    use Class::*;
    match (class(a), class(b)) {
        // Spaces hang at the end of the line before them; kinsoku: no closing mark starts a line,
        // no opening mark ends one.
        (_, Space | Close) | (Open, _) => false,
        (Space | Hyphen | Ideo, _) | (_, Ideo) => true,
        _ => false,
    }
}

/// Whether char `c` is a breakable space (it hangs at a line's end and doesn't count for its width).
pub(crate) fn is_space(c: char) -> bool {
    class(c) == Class::Space
}

/// For each char of `text`, whether it starts a grapheme cluster (a user-perceived character:
/// an emoji sequence, a letter with its combining marks).
pub(crate) fn grapheme_starts(text: &str) -> Vec<bool> {
    if text.is_ascii() && !text.contains('\r') {
        return vec![true; text.len()];
    }
    let mut starts = Vec::with_capacity(text.len());
    for g in text.graphemes(true) {
        starts.extend(g.chars().enumerate().map(|(k, _)| k == 0));
    }
    starts
}

/// The char index of the grapheme cluster boundary after (`forward`) or before char `index` in
/// `text`: where an arrow key, Delete or Backspace moves the caret to.
pub fn grapheme_step(text: &str, index: usize, forward: bool) -> usize {
    let mut at = 0;
    for g in text.graphemes(true) {
        let next = at + g.chars().count();
        if (forward && index < next) || (!forward && index <= next) {
            return if forward { next } else { at };
        }
        at = next;
    }
    at.min(index)
}
