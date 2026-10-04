//! The little bit of Rust lexing the textual gates need.
//!
//! Two gates read source files as text: `mutants` rewrites an operator, and `unsafe-budget` counts a
//! word. Both have to know which bytes are *code*, because `"-"`, `println!("{}", 0 + 1)` and
//! `// unsafe is handled in the store` are text that must not be counted or rewritten. A real parser
//! is the wrong tool here: a mutant is by construction a file that no longer compiles, so the scanner
//! has to work on broken code.
//!
//! Recognised: line comments, block comments (Rust nests them), string literals including the byte and
//! raw forms (`"…"`, `b"…"`, `r"…"`, `r#"…"#`, `br#"…"#`), and character literals (`'x'`, `'\n'`,
//! `'\u{1F600}'`) — while a lifetime (`'a`) is *not* one, which is the trap a scanner that just looks
//! for two quotes falls into. Everything else is [`Class::Code`].
//!
//! The scan is byte-level and never changes a length, so a class list and the text it came from share
//! one set of offsets: a gate that works line by line slices the classes per line and is still right
//! about a block comment that spans several of them.

/// What one byte of a source file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Code: an operator or a word here is part of the program.
    Code,
    /// Inside a comment, including a doc comment.
    Comment,
    /// Inside a string or a character literal.
    Literal,
}

impl Class {
    /// `true` for a byte a gate may read or rewrite as code.
    #[must_use]
    pub fn is_code(self) -> bool {
        self == Class::Code
    }
}

/// Classify every byte of `source`, in order.
///
/// The answer has exactly as many entries as `source` has bytes.
#[must_use]
pub fn classify(source: &str) -> Vec<Class> {
    let bytes = source.as_bytes();
    let mut classes = vec![Class::Code; bytes.len()];
    let mut at = 0usize;
    while at < bytes.len() {
        let Some(byte) = bytes.get(at).copied() else {
            break;
        };
        let (class, end) = if byte == b'/' && bytes.get(at + 1) == Some(&b'/') {
            (Class::Comment, line_comment_end(bytes, at))
        } else if byte == b'/' && bytes.get(at + 1) == Some(&b'*') {
            (Class::Comment, block_comment_end(bytes, at))
        } else if let Some(end) = string_end(bytes, at) {
            (Class::Literal, end)
        } else if byte == b'\'' {
            match char_literal_end(bytes, at) {
                Some(end) => (Class::Literal, end),
                // A lifetime is code, and so is a stray quote in a mutant half-edit: neither is
                // allowed to swallow the rest of the file.
                None => {
                    at += 1;
                    continue;
                }
            }
        } else {
            at += 1;
            continue;
        };
        if let Some(range) = classes.get_mut(at..end) {
            for class_of_byte in range {
                *class_of_byte = class;
            }
        }
        // `end` is always past `at` for a comment or a literal, but a scanner that trusts that is a
        // scanner that hangs on the day it is wrong.
        at = end.max(at + 1);
    }
    classes
}

/// `true` for a byte that can be part of an identifier: ASCII word characters and every non-ASCII
/// byte, so that a rewrite never turns `NotSome(` into `NotNone(`.
#[must_use]
pub fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// End of the line comment starting at `start`: its last byte, not including the newline.
fn line_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start;
    while let Some(byte) = bytes.get(at) {
        if *byte == b'\n' {
            break;
        }
        at += 1;
    }
    at
}

/// End of the block comment starting at `start`, honouring Rust's nesting.
fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start;
    let mut depth = 0usize;
    while at < bytes.len() {
        if bytes.get(at..at + 2) == Some(b"/*") {
            depth += 1;
            at += 2;
            continue;
        }
        if bytes.get(at..at + 2) == Some(b"*/") {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
            continue;
        }
        at += 1;
    }
    // An unterminated comment is what a mutant looks like sometimes; the rest of the file is inside
    // it, which is the conservative answer.
    bytes.len()
}

/// End of the string literal starting at `start`, or `None` when none starts there.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if start > 0
        && bytes
            .get(start - 1)
            .copied()
            .is_some_and(is_identifier_byte)
    {
        // `foo"` is not a string after `foo`; only a token boundary opens one.
        return None;
    }
    let mut at = start;
    if matches!(bytes.get(at), Some(b'b' | b'c')) {
        at += 1;
    }
    if bytes.get(at) == Some(&b'r') {
        at += 1;
        // `rb"…"` is the other spelling of a raw byte string.
        if matches!(bytes.get(at), Some(b'b' | b'c')) {
            at += 1;
        }
        let mut hashes = 0usize;
        while bytes.get(at) == Some(&b'#') {
            hashes += 1;
            at += 1;
        }
        if bytes.get(at) != Some(&b'"') {
            return None;
        }
        return Some(raw_string_end(bytes, at + 1, hashes));
    }
    if bytes.get(at) == Some(&b'"') {
        return Some(escaped_end(bytes, at + 1, b'"'));
    }
    None
}

/// End of the raw string whose content starts at `from` and is closed by `hashes` `#`s and a quote.
fn raw_string_end(bytes: &[u8], from: usize, hashes: usize) -> usize {
    let mut at = from;
    while at < bytes.len() {
        if bytes.get(at) == Some(&b'"')
            && (0..hashes).all(|offset| bytes.get(at + 1 + offset) == Some(&b'#'))
        {
            return at + 1 + hashes;
        }
        at += 1;
    }
    bytes.len()
}

/// End of the literal content starting at `from`, scanning for the first unescaped `quote`.
fn escaped_end(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut at = from;
    while let Some(byte) = bytes.get(at) {
        if *byte == b'\\' {
            at += 2;
            continue;
        }
        if *byte == quote {
            return at + 1;
        }
        at += 1;
    }
    bytes.len()
}

/// End of the character literal starting at `start`, or `None` when `start` opens a lifetime.
fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let content = start + 1;
    match bytes.get(content).copied() {
        // An escape: `'\n'`, `'\''`, `'\u{1F600}'`.
        Some(b'\\') => Some(escaped_end(bytes, content + 2, b'\'')),
        // One character, then the closing quote or not: `'x'` against `'a`.
        Some(_) => {
            let text = std::str::from_utf8(bytes.get(content..)?).ok()?;
            let width = text.chars().next()?.len_utf8();
            (bytes.get(content + width) == Some(&b'\'')).then_some(content + width + 1)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte of `snippet` is classified as `expected` inside `source`.
    fn assert_all(source: &str, snippet: &str, expected: Class) {
        let start = source.find(snippet).expect("the snippet is in the fixture");
        let classes = classify(source);
        let slice = classes
            .get(start..start + snippet.len())
            .expect("the snippet is in range");
        assert!(
            slice.iter().all(|class| *class == expected),
            "`{snippet}` in `{source}` should be {expected:?}: {slice:?}"
        );
    }

    #[test]
    fn a_line_comment_covers_the_rest_of_its_line_and_no_more() {
        let source = "let a = 1; // b / c\nlet b = 2;";
        assert_all(source, "// b / c", Class::Comment);
        assert_all(source, "let b = 2;", Class::Code);
    }

    #[test]
    fn a_block_comment_nests_and_may_span_lines() {
        let source = "a /* one /* two */ three */ b";
        assert_all(source, "/* two */", Class::Comment);
        assert_all(source, "three */", Class::Comment);
        assert_all(source, "b", Class::Code);

        let across = "a /* one\ntwo */ b";
        assert_all(across, "/* one\ntwo */", Class::Comment);
    }

    #[test]
    fn strings_and_their_escapes_are_literals() {
        assert_all(r#"let s = "a \" b";"#, r#""a \" b""#, Class::Literal);
        assert_all("let c = 'x';", "'x'", Class::Literal);
        assert_all(r"let c = '\n';", r"'\n'", Class::Literal);
        assert_all(r"let q = '\'';", r"'\''", Class::Literal);
        assert_all(r"let u = '\u{1F600}';", r"'\u{1F600}'", Class::Literal);
        assert_all("let e = 'é';", "'é'", Class::Literal);
    }

    #[test]
    fn raw_strings_count_their_hashes() {
        assert_all(
            r####"x = r#"a " b"#;"####,
            r##"r#"a " b"#"##,
            Class::Literal,
        );
        assert_all(r####"x = br#"a"#;"####, r##"br#"a"#"##, Class::Literal);
        assert_all("x = r\"a\";", "r\"a\"", Class::Literal);
    }

    #[test]
    fn a_lifetime_is_not_a_character_literal() {
        // The whole point: reading `'a` as an unterminated literal would hide everything after it,
        // which is where a mutation would silently disappear.
        assert_all("fn f<'a>(x: &'a str) {}", "&'a str", Class::Code);
        assert_all("struct Foo<'a> { x: &'a u32 }", "&'a u32", Class::Code);
    }

    #[test]
    fn an_unterminated_literal_swallows_only_the_rest_of_the_file() {
        let source = "let s = \"oops\nx + y";
        assert_all(source, "oops\nx + y", Class::Literal);
    }

    #[test]
    fn an_identifier_byte_is_anything_that_can_be_read_again() {
        assert!(is_identifier_byte(b'a'));
        assert!(is_identifier_byte(b'7'));
        assert!(is_identifier_byte(b'_'));
        assert!(is_identifier_byte(0xE9));
        assert!(!is_identifier_byte(b'+'));
        assert!(!is_identifier_byte(b' '));
    }
}
