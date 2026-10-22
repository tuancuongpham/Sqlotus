//! What the name of a data type says about its values, asked in one place.
//! The catalog, the result grid and people write types alike
//! (`nvarchar(50)`, `numeric(12, 2)`, `double precision`, MySQL's
//! `int unsigned`), and editing rows, test data, variable values and
//! exports need the same answers: is it a number, how long may its text
//! be, does SQL Server need `N'...'` for it.

use crate::Dialect;

/// A data type as written, e.g. `decimal(12, 2) unsigned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlType {
    /// Lower case, without arguments or MySQL's `unsigned` / `zerofill`:
    /// `decimal`, `character varying`, `timestamp`.
    base: String,
    /// The arguments in parentheses: `["12", "2"]`, `["max"]`.
    args: Vec<String>,
    unsigned: bool,
}

const INTEGER: &[&str] = &[
    "int",
    "integer",
    "bigint",
    "smallint",
    "tinyint",
    "mediumint",
    "int2",
    "int4",
    "int8",
    "serial",
    "bigserial",
    "smallserial",
];

/// Inexact numbers: the text read back may differ in the last digits.
const FLOATING: &[&str] = &["float", "real", "double", "double precision", "float4", "float8"];

const EXACT_DECIMAL: &[&str] = &["decimal", "numeric", "dec", "money", "smallmoney"];

const TEXT: &[&str] = &[
    "char",
    "varchar",
    "nchar",
    "nvarchar",
    "text",
    "ntext",
    "character varying",
    "character",
    "tinytext",
    "mediumtext",
    "longtext",
    "citext",
    "bpchar",
];

const BINARY: &[&str] = &[
    "varbinary",
    "binary",
    "image",
    "bytea",
    "blob",
    "tinyblob",
    "mediumblob",
    "longblob",
];

/// Types whose values are written in ASCII only (dates, times, GUIDs).
const ASCII_VALUES: &[&str] = &[
    "date",
    "datetime",
    "datetime2",
    "smalldatetime",
    "time",
    "datetimeoffset",
    "uniqueidentifier",
];

impl SqlType {
    pub fn parse(text: &str) -> Self {
        let (head, args, tail) = match text.split_once('(') {
            Some((head, rest)) => {
                let (inner, tail) = rest.split_once(')').unwrap_or((rest, ""));
                (head, inner.split(',').map(|a| a.trim().to_string()).collect(), tail)
            }
            None => (text, Vec::new(), ""),
        };
        let modifier = |w: &str| matches!(w, "unsigned" | "signed" | "zerofill");
        let words: Vec<String> = head.split_whitespace().map(str::to_ascii_lowercase).collect();
        let kept = words.iter().rposition(|w| !modifier(w)).map_or(0, |p| p + 1);
        let unsigned = words[kept..]
            .iter()
            .map(String::as_str)
            .chain(tail.split_whitespace())
            .any(|w| w.eq_ignore_ascii_case("unsigned"));
        Self {
            base: words[..kept].join(" "),
            args,
            unsigned,
        }
    }

    /// The type without its arguments, lower case: `nvarchar`, `double precision`.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn is(&self, names: &[&str]) -> bool {
        names.contains(&self.base.as_str())
    }

    fn arg(&self, i: usize) -> Option<u32> {
        self.args.get(i)?.parse().ok()
    }

    /// The declared length: `nvarchar(50)` → 50; `max` or none → None.
    pub fn length(&self) -> Option<usize> {
        self.arg(0).map(|n| n as usize)
    }

    /// Digits after the decimal point, when declared: `decimal(12, 2)` → 2.
    pub fn scale(&self) -> Option<u32> {
        self.arg(1)
    }

    /// Digits before the decimal point that fit, when limited:
    /// `decimal(5, 2)` → 3.
    pub fn integer_digits(&self) -> Option<u32> {
        if !self.is(&["decimal", "numeric", "dec", "number"]) {
            return None;
        }
        Some(self.arg(0)?.saturating_sub(self.scale().unwrap_or(0)))
    }

    pub fn is_unsigned(&self) -> bool {
        self.unsigned
    }

    pub fn is_integer(&self) -> bool {
        self.is(INTEGER)
    }

    /// Numbers with a fractional part, exact or not.
    pub fn is_decimal(&self) -> bool {
        self.is(EXACT_DECIMAL) || self.is_floating()
    }

    pub fn is_number(&self) -> bool {
        self.is_integer() || self.is_decimal()
    }

    /// float, real, double: the text read back may differ in the last digits.
    pub fn is_floating(&self) -> bool {
        self.is(FLOATING)
    }

    pub fn is_text(&self) -> bool {
        self.is(TEXT)
    }

    pub fn is_binary(&self) -> bool {
        self.is(BINARY)
    }

    /// SQL Server text in an 8-bit code page (char, varchar, text):
    /// characters outside the code page are lost.
    pub fn is_narrow_text(&self) -> bool {
        self.is(&["char", "varchar", "text"])
    }

    /// SQL Server datetime and smalldatetime read `'YYYY-MM-DD hh:mm'` by
    /// the session's DATEFORMAT (dmy turns January 12 into December 1).
    fn reads_dateformat(&self) -> bool {
        self.is(&["datetime", "smalldatetime"])
    }
}

/// `2024-01-12 10:20:30` → `2024-01-12T10:20:30`: ISO 8601, which every
/// DATEFORMAT and language reads alike. None for anything else.
fn iso_datetime(text: &str) -> Option<String> {
    let (date, time) = text.split_once(' ')?;
    let b = date.as_bytes();
    (b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && date.chars().all(|c| c.is_ascii_digit() || c == '-')
        && time.starts_with(|c: char| c.is_ascii_digit()))
    .then(|| format!("{date}T{time}"))
}

impl Dialect {
    /// `text` as a string literal for a value of type `ty`. SQL Server
    /// takes `N'...'` unless the type holds 8-bit text, or only ASCII
    /// values such as dates, so xml, sysname and the like keep characters
    /// outside the code page; and datetime / smalldatetime in ISO 8601.
    pub fn typed_string(self, text: &str, ty: &SqlType) -> String {
        if self != Dialect::MsSql {
            return self.string_literal(text);
        }
        let iso = ty.reads_dateformat().then(|| iso_datetime(text)).flatten();
        let quoted = self.string_literal(iso.as_deref().unwrap_or(text));
        if ty.is_narrow_text() || ty.is(ASCII_VALUES) {
            quoted
        } else {
            format!("N{quoted}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_as_written() {
        let t = SqlType::parse("decimal(12, 2) unsigned zerofill");
        assert_eq!(
            (t.base(), t.scale(), t.integer_digits(), t.is_unsigned()),
            ("decimal", Some(2), Some(10), true)
        );
        // MySQL 8 writes no display width
        for ty in ["int unsigned", "INT UNSIGNED", "int(11) unsigned", "smallint  unsigned"] {
            let t = SqlType::parse(ty);
            assert!(t.is_integer() && t.is_unsigned(), "{ty}");
        }
        assert!(!SqlType::parse("tinyint(1)").is_unsigned());
        assert_eq!(SqlType::parse("nvarchar(max)").length(), None);
        assert_eq!(SqlType::parse("character varying(20)").length(), Some(20));
        assert_eq!(SqlType::parse("double  precision").base(), "double precision");
        assert_eq!(SqlType::parse("timestamp(6) with time zone").base(), "timestamp");
        assert!(SqlType::parse("float(53)").is_floating() && SqlType::parse("money").is_decimal());
        assert!(SqlType::parse("bytea").is_binary() && SqlType::parse("bpchar(5)").is_text());
        assert_eq!(SqlType::parse("").base(), "");
    }

    #[test]
    fn string_literals_by_type() {
        let ms = Dialect::MsSql;
        let lit = |v: &str, ty: &str| ms.typed_string(v, &SqlType::parse(ty));
        assert_eq!(lit("Việt", "nvarchar(10)"), "N'Việt'");
        assert_eq!(lit("<a>Việt</a>", "xml"), "N'<a>Việt</a>'");
        assert_eq!(lit("O'Brien", "varchar(10)"), "'O''Brien'");
        assert_eq!(lit("2024-01-12 10:20:30.123", "datetime"), "'2024-01-12T10:20:30.123'");
        assert_eq!(lit("2024-01-12 10:20", "smalldatetime"), "'2024-01-12T10:20'");
        assert_eq!(lit("12/01/2024 10:20", "datetime"), "'12/01/2024 10:20'");
        assert_eq!(
            lit("2024-01-12 10:20:30.1234567", "datetime2(7)"),
            "'2024-01-12 10:20:30.1234567'"
        );
        assert_eq!(
            lit("6F9619FF-8B86-D011-B42D-00C04FC964FF", "uniqueidentifier"),
            "'6F9619FF-8B86-D011-B42D-00C04FC964FF'"
        );
        assert_eq!(
            Dialect::MySql.typed_string("C:\\t", &SqlType::parse("varchar(5)")),
            "'C:\\\\t'"
        );
        assert_eq!(
            Dialect::Postgres.typed_string("2024-01-12 10:20", &SqlType::parse("timestamp")),
            "'2024-01-12 10:20'"
        );
    }
}
