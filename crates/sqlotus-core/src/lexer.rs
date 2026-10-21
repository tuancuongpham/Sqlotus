//! Fault-tolerant SQL tokenizer.
//!
//! Every byte of the input ends up in exactly one token, so concatenating the
//! token texts reproduces the source. Unterminated strings, quoted identifiers
//! and comments run to the end of input instead of failing, which matters
//! because the editor tokenizes half-typed queries on every keystroke.

use crate::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Whitespace,
    LineComment,
    BlockComment,
    /// Unquoted identifier or keyword (also `#temp` tables in T-SQL).
    Word,
    /// `[x]`, `"x"` or `` `x` `` depending on dialect.
    QuotedIdent,
    String,
    Number,
    /// `@x`, `@@x` (T-SQL/MySQL).
    Variable,
    /// `$1` (Postgres), `?`.
    Param,
    Operator,
    Comma,
    Semicolon,
    LParen,
    RParen,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub text: &'a str,
    /// Byte offset into the source.
    pub start: usize,
}

impl Token<'_> {
    pub fn end(&self) -> usize {
        self.start + self.text.len()
    }

    pub fn is_trivia(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
        )
    }

    pub fn is_comment(&self) -> bool {
        matches!(self.kind, TokenKind::LineComment | TokenKind::BlockComment)
    }

    /// Words and quoted identifiers, i.e. anything that can name an object.
    pub fn is_name(&self) -> bool {
        matches!(self.kind, TokenKind::Word | TokenKind::QuotedIdent)
    }

    /// The identifier value with dialect quoting removed.
    pub fn ident_value(&self) -> String {
        unquote(self.text)
    }

    pub fn upper(&self) -> String {
        self.text.to_ascii_uppercase()
    }
}

/// Remove identifier quoting (`[a]]b]` -> `a]b`, `"a""b"` -> `a"b`).
pub fn unquote(text: &str) -> String {
    let b = text.as_bytes();
    if b.len() >= 2 {
        // Only slice once both ends are known ASCII quotes: slicing first
        // would cut into multi-byte characters such as the `í` in `ín`.
        let inner = || &text[1..text.len() - 1];
        match (b[0], b[b.len() - 1]) {
            (b'[', b']') => return inner().replace("]]", "]"),
            (b'"', b'"') => return inner().replace("\"\"", "\""),
            (b'`', b'`') => return inner().replace("``", "`"),
            _ => {}
        }
    }
    if let Some(rest) = text.strip_prefix('[') {
        return rest.to_string();
    }
    text.trim_start_matches(['"', '`']).to_string()
}

const MULTI_OPS: &[&str] = &[
    "->>", "#>>", "<=>", "<>", "!=", "<=", ">=", "!<", "!>", "||", "::", "->", "#>", "@>", "<@", "+=", "-=", "*=",
    "/=", "%=", "&=", "|=", "^=", ":=", "=>", "<<", ">>", "&&", "!~", "~*", "?|", "?&",
];

pub fn tokenize(src: &str, dialect: Dialect) -> Vec<Token<'_>> {
    let mut lx = Lexer { src, pos: 0, dialect };
    let mut out = Vec::new();
    while lx.pos < src.len() {
        let start = lx.pos;
        let kind = lx.next_kind();
        debug_assert!(lx.pos > start, "lexer must make progress");
        out.push(Token {
            kind,
            text: &src[start..lx.pos],
            start,
        });
    }
    out
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    dialect: Dialect,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while let Some(c) = self.peek() {
            if !f(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn next_kind(&mut self) -> TokenKind {
        let c = self.peek().expect("not at end");
        let next = self.peek_at(1);
        match c {
            c if c.is_whitespace() => {
                self.eat_while(char::is_whitespace);
                TokenKind::Whitespace
            }
            '-' if next == Some('-') => self.line_comment(),
            '#' if self.dialect == Dialect::MySql => self.line_comment(),
            '/' if next == Some('*') => self.block_comment(),
            '\'' => {
                self.bump();
                self.quoted_body('\'', self.dialect == Dialect::MySql);
                TokenKind::String
            }
            'N' | 'n' if next == Some('\'') && self.dialect != Dialect::Postgres => self.prefixed_string(false),
            'E' | 'e' if next == Some('\'') && self.dialect == Dialect::Postgres => self.prefixed_string(true),
            'B' | 'b' | 'X' | 'x' if next == Some('\'') => self.prefixed_string(false),
            '"' if self.dialect == Dialect::MySql => {
                self.bump();
                self.quoted_body('"', true);
                TokenKind::String
            }
            '"' => {
                self.bump();
                self.quoted_body('"', false);
                TokenKind::QuotedIdent
            }
            '`' if self.dialect == Dialect::MySql => {
                self.bump();
                self.quoted_body('`', false);
                TokenKind::QuotedIdent
            }
            '[' if self.dialect == Dialect::MsSql => {
                self.bump();
                self.quoted_body(']', false);
                TokenKind::QuotedIdent
            }
            '$' if self.dialect == Dialect::Postgres => self.dollar(),
            '@' if self.dialect != Dialect::Postgres && next.is_some_and(is_ident_char_or_at) => {
                self.bump();
                self.eat_while(is_ident_char_or_at);
                TokenKind::Variable
            }
            '#' if self.dialect == Dialect::MsSql && next.is_some_and(|n| is_ident_start(n) || n == '#') => {
                self.bump();
                self.eat_while(|c| is_ident_char(c) || c == '#');
                TokenKind::Word
            }
            c if c.is_ascii_digit() => self.number(),
            '.' if next.is_some_and(|n| n.is_ascii_digit()) => self.number(),
            c if is_ident_start(c) => {
                self.eat_while(is_ident_char);
                TokenKind::Word
            }
            ',' => self.single(TokenKind::Comma),
            ';' => self.single(TokenKind::Semicolon),
            '(' => self.single(TokenKind::LParen),
            ')' => self.single(TokenKind::RParen),
            '.' => self.single(TokenKind::Dot),
            '?' if !matches!(next, Some('|' | '&')) => self.single(TokenKind::Param),
            _ => {
                let rest = self.rest();
                if let Some(op) = MULTI_OPS.iter().find(|op| rest.starts_with(**op)) {
                    self.pos += op.len();
                } else {
                    self.bump();
                }
                TokenKind::Operator
            }
        }
    }

    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.bump();
        kind
    }

    fn line_comment(&mut self) -> TokenKind {
        self.eat_while(|c| c != '\n' && c != '\r');
        TokenKind::LineComment
    }

    fn block_comment(&mut self) -> TokenKind {
        self.pos += 2;
        let nested = self.dialect != Dialect::MySql;
        let mut depth = 1usize;
        while depth > 0 {
            let rest = self.rest();
            if rest.is_empty() {
                break;
            }
            if rest.starts_with("*/") {
                self.pos += 2;
                depth -= 1;
            } else if nested && rest.starts_with("/*") {
                self.pos += 2;
                depth += 1;
            } else {
                self.bump();
            }
        }
        TokenKind::BlockComment
    }

    /// Consume up to and including the closing `quote`; a doubled quote is an
    /// escaped quote. With `backslash`, `\x` escapes any character.
    fn quoted_body(&mut self, quote: char, backslash: bool) {
        while let Some(c) = self.bump() {
            if backslash && c == '\\' {
                self.bump();
            } else if c == quote {
                if self.peek() == Some(quote) {
                    self.bump();
                } else {
                    return;
                }
            }
        }
    }

    fn prefixed_string(&mut self, backslash: bool) -> TokenKind {
        self.pos += 2;
        self.quoted_body('\'', backslash || self.dialect == Dialect::MySql);
        TokenKind::String
    }

    fn dollar(&mut self) -> TokenKind {
        // $1 positional parameter
        if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            self.eat_while(|c| c.is_ascii_digit());
            return TokenKind::Param;
        }
        // $tag$ ... $tag$ dollar-quoted string
        let rest = self.rest();
        let tag_len = rest[1..]
            .find('$')
            .filter(|&i| rest[1..1 + i].chars().all(is_ident_char));
        if let Some(i) = tag_len {
            let tag = &rest[..i + 2];
            let body_start = self.pos + tag.len();
            match self.src[body_start..].find(tag) {
                Some(end) => self.pos = body_start + end + tag.len(),
                None => self.pos = self.src.len(),
            }
            return TokenKind::String;
        }
        self.bump();
        TokenKind::Operator
    }

    fn number(&mut self) -> TokenKind {
        let rest = self.rest();
        if rest.starts_with("0x") || rest.starts_with("0X") {
            self.pos += 2;
            self.eat_while(|c| c.is_ascii_hexdigit());
            return TokenKind::Number;
        }
        self.eat_while(|c| c.is_ascii_digit());
        if self.peek() == Some('.') {
            self.bump();
            self.eat_while(|c| c.is_ascii_digit());
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = matches!(self.peek_at(1), Some('+' | '-'));
            let digit_at = if sign { 2 } else { 1 };
            if self.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += digit_at;
                self.eat_while(|c| c.is_ascii_digit());
            }
        }
        TokenKind::Number
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn is_ident_char_or_at(c: char) -> bool {
    is_ident_char(c) || c == '@' || c == '#'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str, d: Dialect) -> Vec<(TokenKind, &str)> {
        tokenize(src, d)
            .into_iter()
            .filter(|t| !t.is_trivia())
            .map(|t| (t.kind, t.text))
            .collect()
    }

    #[test]
    fn roundtrips_source() {
        let src = "SELECT [a]]b], N'x''y', @v, #t, 1.5e3 FROM dbo.T -- c\n/* x /* y */ z */;";
        let joined: String = tokenize(src, Dialect::MsSql).iter().map(|t| t.text).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn tsql_tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds(
                "SELECT [Order Id], N'ä', @@ROWCOUNT FROM #tmp WHERE x<>1",
                Dialect::MsSql
            ),
            vec![
                (Word, "SELECT"),
                (QuotedIdent, "[Order Id]"),
                (Comma, ","),
                (String, "N'ä'"),
                (Comma, ","),
                (Variable, "@@ROWCOUNT"),
                (Word, "FROM"),
                (Word, "#tmp"),
                (Word, "WHERE"),
                (Word, "x"),
                (Operator, "<>"),
                (Number, "1"),
            ]
        );
    }

    #[test]
    fn postgres_tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds(
                "SELECT x::int, $1, $fn$ body ' $fn$, E'a\\'b' FROM \"T\"",
                Dialect::Postgres
            ),
            vec![
                (Word, "SELECT"),
                (Word, "x"),
                (Operator, "::"),
                (Word, "int"),
                (Comma, ","),
                (Param, "$1"),
                (Comma, ","),
                (String, "$fn$ body ' $fn$"),
                (Comma, ","),
                (String, "E'a\\'b'"),
                (Word, "FROM"),
                (QuotedIdent, "\"T\""),
            ]
        );
    }

    #[test]
    fn mysql_tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds("SELECT `a`, \"s\\\"\" # c\nFROM t", Dialect::MySql),
            vec![
                (Word, "SELECT"),
                (QuotedIdent, "`a`"),
                (Comma, ","),
                (String, "\"s\\\"\""),
                (Word, "FROM"),
                (Word, "t")
            ]
        );
    }

    #[test]
    fn unterminated_input_is_tolerated() {
        for src in ["SELECT 'abc", "SELECT [abc", "/* open", "SELECT \"x"] {
            let joined: String = tokenize(src, Dialect::MsSql).iter().map(|t| t.text).collect();
            assert_eq!(joined, src);
        }
    }

    #[test]
    fn unquote_variants() {
        assert_eq!(unquote("[a]]b]"), "a]b");
        assert_eq!(unquote("\"a\"\"b\""), "a\"b");
        assert_eq!(unquote("`x`"), "x");
        assert_eq!(unquote("[open"), "open");
        assert_eq!(unquote("plain"), "plain");
    }
}
