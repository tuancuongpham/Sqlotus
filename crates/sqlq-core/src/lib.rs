//! Pure SQL tooling: tokenizer, formatter, completion and snippets.
//!
//! Nothing in this crate performs IO; database metadata comes in through
//! [`catalog::Catalog`], which the `sqlq-db` crate fills.

pub mod catalog;
pub mod completion;
pub mod format;
pub mod keywords;
pub mod lexer;
pub mod scope;
pub mod search;
pub mod snippet;
pub mod statements;

use serde::{Deserialize, Serialize};

/// SQL dialect the text is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    #[default]
    MsSql,
    Postgres,
    MySql,
}

impl Dialect {
    /// Quote an identifier for this dialect when it needs quoting.
    pub fn quote_ident(self, name: &str) -> String {
        if !self.needs_quoting(name) {
            return name.to_string();
        }
        match self {
            Dialect::MsSql => format!("[{}]", name.replace(']', "]]")),
            Dialect::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
            Dialect::MySql => format!("`{}`", name.replace('`', "``")),
        }
    }

    pub fn needs_quoting(self, name: &str) -> bool {
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return true;
        };
        let first_ok =
            first.is_alphabetic() || first == '_' || (self == Dialect::MsSql && first == '#');
        if !first_ok
            || !name
                .chars()
                .skip(1)
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            return true;
        }
        if self == Dialect::Postgres && name.chars().any(|c| c.is_uppercase()) {
            return true;
        }
        keywords::is_reserved(&name.to_ascii_uppercase(), self)
    }
}

pub use catalog::Catalog;
pub use completion::{
    CompletionItem, CompletionKind, CompletionOptions, CompletionResult, complete,
};
pub use format::{FormatOptions, format_sql};
pub use lexer::{Token, TokenKind, tokenize};

/// Convert a UTF-16 code unit offset (as used by JavaScript/Monaco) to a
/// byte offset into `s`.
pub fn utf16_to_byte(s: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// Convert a byte offset into `s` to a UTF-16 code unit offset.
pub fn byte_to_utf16(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_roundtrip() {
        let s = "a\u{1F600}bé";
        assert_eq!(utf16_to_byte(s, 3), 5);
        assert_eq!(byte_to_utf16(s, 5), 3);
        assert_eq!(utf16_to_byte(s, 99), s.len());
    }

    #[test]
    fn quoting() {
        assert_eq!(Dialect::MsSql.quote_ident("Order"), "[Order]");
        assert_eq!(Dialect::MsSql.quote_ident("Name"), "Name");
        assert_eq!(Dialect::Postgres.quote_ident("Name"), "\"Name\"");
        assert_eq!(Dialect::MySql.quote_ident("my col"), "`my col`");
    }
}

#[cfg(test)]
mod ime_tests {
    use super::*;

    /// Vietnamese IMEs (Telex) rewrite the word being typed: `ins` becomes
    /// `ín`/`Ín`, another `s` turns it back. Every intermediate text and
    /// every cursor position must be handled without panicking.
    #[test]
    fn vietnamese_ime_intermediate_states_do_not_panic() {
        let texts = [
            "i",
            "in",
            "ín",
            "Ín",
            "ins",
            "Ins",
            "Ínsert",
            "insẻt",
            "SELECT * FROM t WHERE x = 'ín' AND Ín",
            "SELECT ín. FROM Ín ín",
            "-- ghi chú tiếng Việt\nÍn",
            "Nguyễn Văn A",
            "ƯƠ ươ ẫ ặ 😀 Ín",
        ];
        let catalog = Catalog::default();
        for text in texts {
            let len16 = byte_to_utf16(text, text.len());
            for off16 in 0..=len16 + 1 {
                let off = utf16_to_byte(text, off16);
                for d in [Dialect::MsSql, Dialect::Postgres, Dialect::MySql] {
                    let r = complete(
                        text,
                        off,
                        d,
                        Some(&catalog),
                        &crate::snippet::builtin_snippets(),
                        &CompletionOptions::default(),
                    );
                    assert!(r.replace_start <= r.replace_end && r.replace_end <= text.len());
                    let _ = statements::statement_at(text, d, off);
                    let _ = format_sql(text, d, &FormatOptions::default());
                }
            }
            // raw byte offsets inside a multi-byte char
            for off in 0..=text.len() + 2 {
                let _ = complete(
                    text,
                    off,
                    Dialect::MsSql,
                    None,
                    &[],
                    &CompletionOptions::default(),
                );
                let _ = statements::statement_at(text, Dialect::MsSql, off);
            }
        }
    }
}
