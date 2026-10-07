// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The in-tree JavaScript minifier.
//!
//! A single tokenising pass. String, template and regex literals are
//! copied through byte for byte; only the whitespace and comments between
//! tokens are touched. The earlier two-pass version tracked quotes in its
//! first pass only, so the second collapsed whitespace inside literals:
//! `"0px 0px -15% 0px"` became `"0px 0px-15%0px"`, `'$ '` became `'$'`,
//! template literals lost their indentation, and a regex such as `/[/*]/`
//! opened a block comment that swallowed the rest of the script (#799).

/// Keywords after which `/` starts a regular expression, not a division.
const REGEX_PREFIX_KEYWORDS: &[&str] = &[
    "return",
    "typeof",
    "case",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "do",
    "else",
    "yield",
    "await",
    "instanceof",
];

/// Minifies JavaScript by removing comments and collapsing whitespace.
///
/// ssg's own implementation rather than a dependency. It is deliberately
/// conservative: it does not rename, reorder or rewrite anything, so it
/// cannot change what a script does. String, template and regex literals
/// are copied verbatim, and the division operator is left alone.
///
/// # Examples
///
/// ```
/// use ssg::plugins::minify_js;
///
/// let out = minify_js("const m = { rootMargin: \"0px 0px -15% 0px\" };");
/// assert_eq!(out, "const m={rootMargin:\"0px 0px -15% 0px\"};");
/// ```
#[must_use]
pub fn minify_js(js: &str) -> String {
    let src: Vec<char> = js.chars().collect();
    let mut m = Minifier {
        src: &src,
        out: String::with_capacity(js.len()),
        gap: Gap::None,
        after_literal: false,
    };
    let mut i = 0;
    while i < src.len() {
        i = m.step(i);
    }
    m.out
}

/// Whitespace pending between two tokens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gap {
    None,
    Space,
    Newline,
}

struct Minifier<'a> {
    src: &'a [char],
    out: String,
    gap: Gap,
    /// The last token emitted was a string, template or regex literal.
    after_literal: bool,
}

impl Minifier<'_> {
    /// Consumes the token at `i` and returns the index after it.
    fn step(&mut self, i: usize) -> usize {
        let c = self.src[i];
        let next = self.src.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                skip_while(self.src, i, |c| !is_line_break(c))
            }
            '/' if next == Some('*') => self.skip_block_comment(i),
            '\'' | '"' => self.copy_literal(i, string_end(self.src, i)),
            '`' => self.copy_literal(i, template_end(self.src, i)),
            '/' if self.regex_allowed() => {
                self.copy_literal(i, regex_end(self.src, i))
            }
            c if c.is_whitespace() => {
                self.widen_gap(is_line_break(c));
                i + 1
            }
            _ => {
                self.flush_gap(c);
                self.out.push(c);
                self.after_literal = false;
                i + 1
            }
        }
    }

    /// Records whitespace; a line break outranks a space because it can
    /// end a statement.
    fn widen_gap(&mut self, line_break: bool) {
        if line_break {
            self.gap = Gap::Newline;
        } else if self.gap == Gap::None {
            self.gap = Gap::Space;
        }
    }

    /// A block comment separates tokens like the whitespace it contains.
    fn skip_block_comment(&mut self, i: usize) -> usize {
        let mut j = i + 2;
        while j < self.src.len()
            && !(self.src[j] == '*' && self.src.get(j + 1) == Some(&'/'))
        {
            j += 1;
        }
        let breaks = self.src[i..j.min(self.src.len())]
            .iter()
            .any(|&c| is_line_break(c));
        self.widen_gap(breaks);
        (j + 2).min(self.src.len())
    }

    fn copy_literal(&mut self, start: usize, end: usize) -> usize {
        self.flush_gap(self.src[start]);
        self.out.extend(&self.src[start..end]);
        self.after_literal = true;
        end
    }

    /// Emits the pending whitespace if dropping it would join two tokens.
    fn flush_gap(&mut self, next: char) {
        let gap = std::mem::replace(&mut self.gap, Gap::None);
        let Some(prev) = self.out.chars().next_back() else {
            return;
        };
        if gap != Gap::None && needs_separator(prev, next) {
            self.out.push(if gap == Gap::Newline { '\n' } else { ' ' });
        }
    }

    /// Whether a `/` here opens a regex: after an operator, an opening
    /// bracket, a keyword or nothing at all. After a value (an
    /// identifier, a number, a literal, `)`, `]` or `}`) it divides.
    fn regex_allowed(&self) -> bool {
        if self.after_literal {
            return false;
        }
        match self.out.chars().next_back() {
            None => true,
            Some(c) if is_word(c) => {
                REGEX_PREFIX_KEYWORDS.contains(&trailing_word(&self.out))
            }
            Some(')' | ']' | '}') => false,
            // `a++ / b`: a postfix operator ends a value.
            Some(_) => !(self.out.ends_with("++") || self.out.ends_with("--")),
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

const fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Whether whitespace between `prev` and `next` is significant: two words
/// would merge, `a - -b` would become a decrement, `a / /re/` a comment,
/// and `1 .toString()` a malformed number.
fn needs_separator(prev: char, next: char) -> bool {
    (is_word(prev) && is_word(next))
        || (prev == next && matches!(prev, '+' | '-' | '/'))
        || (prev.is_ascii_digit() && next == '.')
}

/// The identifier or number at the end of `out`.
fn trailing_word(out: &str) -> &str {
    let start = out
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(out.len(), |(i, _)| i);
    &out[start..]
}

/// The first index at or after `i` whose character fails `keep`.
fn skip_while(src: &[char], i: usize, keep: impl Fn(char) -> bool) -> usize {
    src[i..]
        .iter()
        .position(|&c| !keep(c))
        .map_or(src.len(), |n| i + n)
}

/// Index just past the string literal opening at `i`. An unterminated
/// string stops at the line break, which JavaScript forbids inside one.
const fn string_end(src: &[char], i: usize) -> usize {
    let quote = src[i];
    let mut j = i + 1;
    while j < src.len() {
        match src[j] {
            '\\' => j += 2,
            c if c == quote => return j + 1,
            c if is_line_break(c) => return j,
            _ => j += 1,
        }
    }
    src.len()
}

/// Index just past the template literal opening at `i`, stepping over
/// every `${…}` substitution, which may hold strings and templates of its
/// own.
fn template_end(src: &[char], i: usize) -> usize {
    let mut j = i + 1;
    while j < src.len() {
        match src[j] {
            '\\' => j += 2,
            '`' => return j + 1,
            '$' if src.get(j + 1) == Some(&'{') => {
                j = substitution_end(src, j + 2);
            }
            _ => j += 1,
        }
    }
    src.len()
}

/// Index just past the `}` closing a `${` substitution whose body starts
/// at `i`.
fn substitution_end(src: &[char], i: usize) -> usize {
    let mut depth = 1usize;
    let mut j = i;
    while j < src.len() {
        match src[j] {
            '{' => depth += 1,
            '}' if depth == 1 => return j + 1,
            '}' => depth -= 1,
            '\'' | '"' => j = string_end(src, j) - 1,
            '`' => j = template_end(src, j) - 1,
            _ => {}
        }
        j += 1;
    }
    src.len()
}

/// Index just past the regex literal opening at `i`. A `/` inside a
/// character class does not close it; a line break ends the scan, since
/// a regex cannot span lines.
const fn regex_end(src: &[char], i: usize) -> usize {
    let mut j = i + 1;
    let mut in_class = false;
    while j < src.len() {
        match src[j] {
            '\\' => j += 1,
            '[' => in_class = true,
            ']' => in_class = false,
            '/' if !in_class => return j + 1,
            c if is_line_break(c) => return j,
            _ => {}
        }
        j += 1;
    }
    src.len()
}

#[cfg(test)]
#[path = "js_minify_tests.rs"]
mod tests;
