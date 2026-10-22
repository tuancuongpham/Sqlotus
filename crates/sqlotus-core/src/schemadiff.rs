//! Schema compare: the differences between two databases (tables, columns,
//! keys, indexes, foreign keys, checks, views, routines, triggers) and a
//! script that makes the target match the source. The script is for review:
//! operations that can lose data are listed as warnings at the top.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::ObjectKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ColumnDef {
    pub name: String,
    /// Full type as written in DDL: `nvarchar(100)`, `decimal(12, 2)`.
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    /// Name of the default constraint (SQL Server).
    pub default_name: Option<String>,
    pub identity: bool,
    /// Expression of a computed / generated column.
    pub computed: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeyDef {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexDef {
    pub name: String,
    /// Key columns, with ` DESC` appended where descending.
    pub columns: Vec<String>,
    pub include: Vec<String>,
    pub unique: bool,
    pub clustered: bool,
    pub filter: Option<String>,
    /// Exact CREATE statement when the server provides one (Postgres).
    pub definition: Option<String>,
    /// A UNIQUE constraint (dropped and added as a constraint, not an index).
    pub constraint: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ForeignKeyDef {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// `CASCADE`, `SET NULL`, `SET DEFAULT`, `RESTRICT`; None = no action.
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CheckDef {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TableDef {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: Option<KeyDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub checks: Vec<CheckDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleDef {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    /// Executable CREATE statement.
    pub definition: String,
    /// Table of a trigger (needed to drop it in Postgres).
    #[serde(default)]
    pub table: Option<String>,
}

/// Everything schema compare looks at in one database.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SchemaModel {
    pub tables: Vec<TableDef>,
    pub modules: Vec<ModuleDef>,
    /// Views / routines / triggers whose definition the login cannot read
    /// (`schema.name`). They are left out of the comparison: treating them
    /// as missing would drop them from the target.
    pub unreadable: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Change {
    /// Only in the source: will be created in the target.
    Added,
    /// Only in the target: will be dropped.
    Removed,
    /// In both, different: will be changed.
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectDiff {
    /// Stable id for selecting the object (`table:dbo.orders`).
    pub key: String,
    pub kind: ObjectKind,
    pub schema: String,
    pub name: String,
    pub change: Change,
    /// What differs, one line each.
    pub details: Vec<String>,
    /// Comparable text of each side (for a side-by-side diff).
    pub source_text: Option<String>,
    pub target_text: Option<String>,
    /// Data-loss or manual-work notes for applying this change.
    pub warnings: Vec<String>,
}

/// An object by name, for lists of identical objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub key: String,
    pub kind: ObjectKind,
    pub schema: String,
    pub name: String,
}

impl ObjectRef {
    fn new(kind: ObjectKind, schema: &str, name: &str) -> Self {
        Self {
            key: object_key(kind, schema, name),
            kind,
            schema: schema.into(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDiff {
    pub differences: Vec<ObjectDiff>,
    /// Objects present in both and identical.
    pub identical: usize,
    /// The identical objects themselves (sorted like the differences).
    pub identical_objects: Vec<ObjectRef>,
    /// Objects not compared because one side could not read them.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompareOptions {
    /// Ignore differences in whitespace inside definitions.
    pub ignore_whitespace: bool,
    /// Ignore upper/lower case in definitions.
    pub ignore_case: bool,
    /// Match object names case-insensitively (SQL Server, MySQL on Windows).
    pub case_insensitive_names: bool,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: true,
            ignore_case: false,
            case_insensitive_names: true,
        }
    }
}

// ---------------------------------------------------------------------------
// normalisation
// ---------------------------------------------------------------------------

fn name_key(s: &str, opts: &CompareOptions) -> String {
    if opts.case_insensitive_names {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn norm_type(t: &str) -> String {
    t.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect()
}

/// `((0))` â†’ `0`, `(getdate())` â†’ `getdate()`: SQL Server wraps defaults.
fn norm_default(d: &Option<String>) -> Option<String> {
    let mut s: String = d
        .as_ref()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    while s.starts_with('(') && s.ends_with(')') && balanced(&s[1..s.len() - 1]) {
        s = s[1..s.len() - 1].to_string();
    }
    Some(s)
}

fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn norm_text(s: &str, opts: &CompareOptions) -> String {
    let t = if opts.ignore_whitespace {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches(';')
            .trim()
            .to_string()
    } else {
        s.replace("\r\n", "\n").trim_end().to_string()
    };
    if opts.ignore_case { t.to_lowercase() } else { t }
}

/// Module text for comparing: `CREATE OR ALTER` / `CREATE OR REPLACE` /
/// `ALTER` at the start mean the same object as `CREATE` (SQL Server keeps
/// the header exactly as it was run).
fn norm_module(s: &str, opts: &CompareOptions) -> String {
    let t = strip_leading_comments(s);
    let words: Vec<&str> = t.splitn(4, char::is_whitespace).filter(|w| !w.is_empty()).collect();
    let up: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let skip = match up.as_slice() {
        [c, o, a, ..] if c == "CREATE" && o == "OR" && (a == "ALTER" || a == "REPLACE") => Some(3),
        [a, ..] if a == "ALTER" || a == "CREATE" => Some(1),
        _ => None,
    };
    let body = match skip {
        Some(k) => {
            let mut rest = t;
            for _ in 0..k {
                rest = rest.trim_start();
                rest = &rest[rest.find(char::is_whitespace).unwrap_or(rest.len())..];
            }
            format!("CREATE {}", rest.trim_start())
        }
        None => t.to_string(),
    };
    norm_text(&body, opts)
}

/// Text after any comments and whitespace at the start (SQL Server keeps
/// comments that preceded CREATE in the same batch as part of the module).
fn strip_leading_comments(s: &str) -> &str {
    let mut rest = s.trim_start();
    loop {
        if rest.starts_with("--") {
            rest = rest[rest.find('\n').map(|i| i + 1).unwrap_or(rest.len())..].trim_start();
        } else if rest.starts_with("/*") {
            rest = rest[rest.find("*/").map(|i| i + 2).unwrap_or(rest.len())..].trim_start();
        } else {
            return rest;
        }
    }
}

/// Parts of a table that are matched by name between two databases.
trait Named {
    fn name(&self) -> &str;
}

macro_rules! named_by_field {
    ($($t:ty),*) => {
        $(impl Named for $t {
            fn name(&self) -> &str {
                &self.name
            }
        })*
    };
}

named_by_field!(ColumnDef, IndexDef, ForeignKeyDef, CheckDef);

/// The first of `items` called `name`.
fn named<'x, T: Named>(items: &'x [T], name: &str, opts: &CompareOptions) -> Option<&'x T> {
    let key = name_key(name, opts);
    items.iter().find(|x| name_key(x.name(), opts) == key)
}

fn norm_cols(c: &[String]) -> Vec<String> {
    c.iter().map(|x| x.to_lowercase()).collect()
}

fn kind_word(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::Procedure => "procedure",
        ObjectKind::ScalarFunction | ObjectKind::TableFunction => "function",
        ObjectKind::Trigger => "trigger",
        ObjectKind::Synonym => "synonym",
        ObjectKind::Sequence => "sequence",
    }
}

/// Stable id of an object: `table:dbo.orders` (names lower-cased).
pub fn object_key(kind: ObjectKind, schema: &str, name: &str) -> String {
    format!("{}:{}.{}", kind_word(kind), schema.to_lowercase(), name.to_lowercase())
}

// ---------------------------------------------------------------------------
// DDL text
// ---------------------------------------------------------------------------

fn q(d: Dialect, s: &str) -> String {
    d.quote_ident(s)
}

fn qualified(d: Dialect, schema: &str, name: &str) -> String {
    if schema.is_empty() {
        q(d, name)
    } else {
        format!("{}.{}", q(d, schema), q(d, name))
    }
}

fn column_sql(c: &ColumnDef, d: Dialect) -> String {
    let mut s = format!("{} ", q(d, &c.name));
    if let Some(expr) = &c.computed {
        return match d {
            Dialect::MsSql => format!("{}AS {expr}", s),
            Dialect::Postgres => format!("{s}{} GENERATED ALWAYS AS ({expr}) STORED", c.data_type),
            Dialect::MySql => format!("{s}{} GENERATED ALWAYS AS ({expr})", c.data_type),
        };
    }
    s.push_str(&c.data_type);
    if c.identity {
        s.push_str(match d {
            Dialect::MsSql => " IDENTITY(1, 1)",
            Dialect::Postgres => " GENERATED BY DEFAULT AS IDENTITY",
            Dialect::MySql => " AUTO_INCREMENT",
        });
    }
    s.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
    if let Some(def) = &c.default
        && !(d == Dialect::Postgres && c.identity)
    {
        match (&c.default_name, d) {
            (Some(n), Dialect::MsSql) => s.push_str(&format!(" CONSTRAINT {} DEFAULT {def}", q(d, n))),
            _ => s.push_str(&format!(" DEFAULT {def}")),
        }
    }
    s
}

fn index_sql(t: &TableDef, i: &IndexDef, d: Dialect) -> String {
    if i.constraint {
        let cols = i
            .columns
            .iter()
            .map(|c| quote_index_col(c, d))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "ALTER TABLE {} ADD CONSTRAINT {} UNIQUE ({cols});",
            qualified(d, &t.schema, &t.name),
            q(d, &i.name)
        );
    }
    if let Some(def) = &i.definition {
        return format!("{};", def.trim_end_matches(';'));
    }
    let cols = i
        .columns
        .iter()
        .map(|c| quote_index_col(c, d))
        .collect::<Vec<_>>()
        .join(", ");
    let mut s = format!(
        "CREATE {}{}INDEX {} ON {} ({cols})",
        if i.unique { "UNIQUE " } else { "" },
        match (d, i.clustered) {
            (Dialect::MsSql, true) => "CLUSTERED ",
            (Dialect::MsSql, false) => "NONCLUSTERED ",
            _ => "",
        },
        q(d, &i.name),
        qualified(d, &t.schema, &t.name)
    );
    if !i.include.is_empty() && d != Dialect::MySql {
        s.push_str(&format!(
            " INCLUDE ({})",
            i.include.iter().map(|c| q(d, c)).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(f) = &i.filter {
        s.push_str(&format!(" WHERE {f}"));
    }
    s.push(';');
    s
}

fn quote_index_col(c: &str, d: Dialect) -> String {
    match c.strip_suffix(" DESC") {
        Some(name) => format!("{} DESC", q(d, name)),
        None => q(d, c),
    }
}

fn fk_sql(t: &TableDef, f: &ForeignKeyDef, d: Dialect) -> String {
    let mut s = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        qualified(d, &t.schema, &t.name),
        q(d, &f.name),
        f.columns.iter().map(|c| q(d, c)).collect::<Vec<_>>().join(", "),
        qualified(d, &f.ref_schema, &f.ref_table),
        f.ref_columns.iter().map(|c| q(d, c)).collect::<Vec<_>>().join(", ")
    );
    if let Some(a) = &f.on_delete {
        s.push_str(&format!(" ON DELETE {a}"));
    }
    if let Some(a) = &f.on_update {
        s.push_str(&format!(" ON UPDATE {a}"));
    }
    s.push(';');
    s
}

fn check_sql(t: &TableDef, c: &CheckDef, d: Dialect) -> String {
    let def = c.definition.trim();
    let body = if def.to_ascii_uppercase().starts_with("CHECK") {
        def.to_string()
    } else {
        format!("CHECK {def}")
    };
    format!(
        "ALTER TABLE {} ADD CONSTRAINT {} {body};",
        qualified(d, &t.schema, &t.name),
        q(d, &c.name)
    )
}

fn pk_clause(t: &TableDef, d: Dialect) -> Option<String> {
    let pk = t.primary_key.as_ref()?;
    let cols = pk.columns.iter().map(|c| q(d, c)).collect::<Vec<_>>().join(", ");
    Some(if d == Dialect::MySql || pk.name.is_empty() {
        format!("PRIMARY KEY ({cols})")
    } else {
        format!("CONSTRAINT {} PRIMARY KEY ({cols})", q(d, &pk.name))
    })
}

/// CREATE TABLE with its key; indexes, foreign keys and checks follow as
/// separate statements (also used as the comparable text of a table).
pub fn table_sql(t: &TableDef, d: Dialect, with_dependents: bool) -> String {
    let mut lines: Vec<String> = t.columns.iter().map(|c| format!("    {}", column_sql(c, d))).collect();
    if let Some(pk) = pk_clause(t, d) {
        lines.push(format!("    {pk}"));
    }
    let mut s = format!(
        "CREATE TABLE {} (\n{}\n);",
        qualified(d, &t.schema, &t.name),
        lines.join(",\n")
    );
    if with_dependents {
        let mut idx: Vec<&IndexDef> = t.indexes.iter().collect();
        idx.sort_by_key(|i| i.name.to_lowercase());
        for i in idx {
            s.push('\n');
            s.push_str(&index_sql(t, i, d));
        }
        let mut fks: Vec<&ForeignKeyDef> = t.foreign_keys.iter().collect();
        fks.sort_by_key(|f| f.name.to_lowercase());
        for f in fks {
            s.push('\n');
            s.push_str(&fk_sql(t, f, d));
        }
        let mut checks: Vec<&CheckDef> = t.checks.iter().collect();
        checks.sort_by_key(|c| c.name.to_lowercase());
        for c in checks {
            s.push('\n');
            s.push_str(&check_sql(t, c, d));
        }
    }
    s
}

// ---------------------------------------------------------------------------
// comparing
// ---------------------------------------------------------------------------

fn same_index(a: &IndexDef, b: &IndexDef) -> bool {
    if let (Some(x), Some(y)) = (&a.definition, &b.definition) {
        let o = CompareOptions {
            ignore_case: true,
            ..Default::default()
        };
        return a.constraint == b.constraint && norm_text(x, &o) == norm_text(y, &o);
    }
    a.constraint == b.constraint
        && norm_cols(&a.columns) == norm_cols(&b.columns)
        && {
            let mut x = norm_cols(&a.include);
            let mut y = norm_cols(&b.include);
            x.sort();
            y.sort();
            x == y
        }
        && a.unique == b.unique
        && a.clustered == b.clustered
        && a.filter.as_deref().map(|f| {
            norm_text(
                f,
                &CompareOptions {
                    ignore_case: true,
                    ..Default::default()
                },
            )
        }) == b.filter.as_deref().map(|f| {
            norm_text(
                f,
                &CompareOptions {
                    ignore_case: true,
                    ..Default::default()
                },
            )
        })
}

fn same_fk(a: &ForeignKeyDef, b: &ForeignKeyDef) -> bool {
    norm_cols(&a.columns) == norm_cols(&b.columns)
        && a.ref_table.eq_ignore_ascii_case(&b.ref_table)
        && a.ref_schema.eq_ignore_ascii_case(&b.ref_schema)
        && norm_cols(&a.ref_columns) == norm_cols(&b.ref_columns)
        && a.on_delete.as_deref().map(str::to_uppercase) == b.on_delete.as_deref().map(str::to_uppercase)
        && a.on_update.as_deref().map(str::to_uppercase) == b.on_update.as_deref().map(str::to_uppercase)
}

fn norm_check(s: &str) -> String {
    let mut t: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    if let Some(rest) = t.strip_prefix("check") {
        t = rest.to_string();
    }
    while t.starts_with('(') && t.ends_with(')') && balanced(&t[1..t.len() - 1]) {
        t = t[1..t.len() - 1].to_string();
    }
    t
}

/// Size of a type for narrowing checks: (family, length/precision, scale).
fn type_size(t: &str) -> (String, Option<i64>, Option<i64>) {
    let n = norm_type(t);
    let base = n.split('(').next().unwrap_or("").to_string();
    let args: Vec<Option<i64>> = n
        .split_once('(')
        .map(|(_, r)| {
            r.trim_end_matches(')')
                .split(',')
                .map(|x| if x == "max" { Some(i64::MAX) } else { x.parse().ok() })
                .collect()
        })
        .unwrap_or_default();
    let family = match base.as_str() {
        "nvarchar" | "varchar" | "nchar" | "char" | "charactervarying" | "character" | "text" | "ntext" => {
            "text".to_string()
        }
        "tinyint" => "int1".into(),
        "smallint" | "int2" => "int2".into(),
        "int" | "integer" | "int4" | "mediumint" => "int4".into(),
        "bigint" | "int8" => "int8".into(),
        other => other.to_string(),
    };
    (family, args.first().copied().flatten(), args.get(1).copied().flatten())
}

/// Why changing a column from `from` (target) to `to` (source) can lose data.
fn narrowing(from: &str, to: &str) -> Option<String> {
    let (ff, fl, fs) = type_size(from);
    let (tf, tl, ts) = type_size(to);
    let int_rank = |f: &str| ["int1", "int2", "int4", "int8"].iter().position(|x| *x == f);
    if let (Some(a), Some(b)) = (int_rank(&ff), int_rank(&tf)) {
        return (b < a).then(|| format!("{from} â†’ {to} is smaller: values out of range fail"));
    }
    if ff != tf {
        return Some(format!(
            "{from} â†’ {to} changes the kind of data: values that do not convert fail"
        ));
    }
    if let (Some(a), Some(b)) = (fl, tl)
        && b < a
    {
        return Some(format!("{from} â†’ {to} is shorter: longer values fail or are cut"));
    }
    if let (Some(a), Some(b)) = (fs, ts)
        && b < a
    {
        return Some(format!("{from} â†’ {to} keeps fewer decimals: values are rounded"));
    }
    None
}

fn compare_tables(s: &TableDef, t: &TableDef, opts: &CompareOptions) -> (Vec<String>, Vec<String>) {
    let mut details = Vec::new();
    let mut warnings = Vec::new();
    for c in &s.columns {
        match named(&t.columns, &c.name, opts) {
            None => {
                details.push(format!("column {} {} added", c.name, c.data_type));
                if !c.nullable && c.default.is_none() && !c.identity && c.computed.is_none() {
                    warnings.push(format!(
                        "New column {} is NOT NULL without a default: adding it fails if {} has rows.",
                        c.name, t.name
                    ));
                }
            }
            Some(o) => {
                if norm_type(&c.data_type) != norm_type(&o.data_type) {
                    details.push(format!("column {}: type {} â†’ {}", c.name, o.data_type, c.data_type));
                    if let Some(w) = narrowing(&o.data_type, &c.data_type) {
                        warnings.push(format!("Column {}.{}: {w}.", t.name, c.name));
                    }
                }
                if c.nullable != o.nullable {
                    details.push(format!(
                        "column {}: {} â†’ {}",
                        c.name,
                        if o.nullable { "NULL" } else { "NOT NULL" },
                        if c.nullable { "NULL" } else { "NOT NULL" }
                    ));
                    if !c.nullable {
                        warnings.push(format!(
                            "Column {}.{} becomes NOT NULL: fails if it holds NULLs.",
                            t.name, c.name
                        ));
                    }
                }
                if norm_default(&c.default) != norm_default(&o.default) {
                    details.push(format!(
                        "column {}: default {} â†’ {}",
                        c.name,
                        o.default.as_deref().unwrap_or("none"),
                        c.default.as_deref().unwrap_or("none")
                    ));
                }
                if c.identity != o.identity {
                    details.push(format!("column {}: identity {} â†’ {}", c.name, o.identity, c.identity));
                    warnings.push(format!("Column {}.{}: identity cannot be changed in place; the script leaves it for you to do by hand.", t.name, c.name));
                }
                if c.computed.as_deref().map(|x| norm_text(x, opts))
                    != o.computed.as_deref().map(|x| norm_text(x, opts))
                {
                    details.push(format!("column {}: computed expression differs", c.name));
                }
            }
        }
    }
    for o in &t.columns {
        if named(&s.columns, &o.name, opts).is_none() {
            details.push(format!("column {} removed", o.name));
            warnings.push(format!("Column {}.{} is dropped: its data is lost.", t.name, o.name));
        }
    }
    let src_order: Vec<String> = s.columns.iter().map(|c| name_key(&c.name, opts)).collect();
    let tgt_order: Vec<String> = t
        .columns
        .iter()
        .map(|c| name_key(&c.name, opts))
        .filter(|n| src_order.contains(n))
        .collect();
    let src_common: Vec<String> = src_order.iter().filter(|n| tgt_order.contains(n)).cloned().collect();
    if src_common != tgt_order {
        details.push("column order differs (not changed by the script)".into());
    }
    let pk_cols = |k: &Option<KeyDef>| k.as_ref().map(|k| norm_cols(&k.columns));
    if pk_cols(&s.primary_key) != pk_cols(&t.primary_key) {
        details.push(format!(
            "primary key ({}) â†’ ({})",
            t.primary_key.as_ref().map(|k| k.columns.join(", ")).unwrap_or_default(),
            s.primary_key.as_ref().map(|k| k.columns.join(", ")).unwrap_or_default()
        ));
    }
    for i in &s.indexes {
        match named(&t.indexes, &i.name, opts) {
            None => details.push(format!("index {} added", i.name)),
            Some(o) if !same_index(i, o) => details.push(format!("index {} changed", i.name)),
            _ => {}
        }
    }
    for o in &t.indexes {
        if named(&s.indexes, &o.name, opts).is_none() {
            details.push(format!("index {} removed", o.name));
        }
    }
    for f in &s.foreign_keys {
        match named(&t.foreign_keys, &f.name, opts) {
            None => details.push(format!("foreign key {} added", f.name)),
            Some(o) if !same_fk(f, o) => details.push(format!("foreign key {} changed", f.name)),
            _ => {}
        }
    }
    for o in &t.foreign_keys {
        if named(&s.foreign_keys, &o.name, opts).is_none() {
            details.push(format!("foreign key {} removed", o.name));
        }
    }
    for c in &s.checks {
        match named(&t.checks, &c.name, opts) {
            None => details.push(format!("check {} added", c.name)),
            Some(o) if norm_check(&c.definition) != norm_check(&o.definition) => {
                details.push(format!("check {} changed", c.name))
            }
            _ => {}
        }
    }
    for o in &t.checks {
        if named(&s.checks, &o.name, opts).is_none() {
            details.push(format!("check {} removed", o.name));
        }
    }
    (details, warnings)
}

/// Differences that make `target` differ from `source`.
pub fn compare(source: &SchemaModel, target: &SchemaModel, dialect: Dialect, opts: &CompareOptions) -> SchemaDiff {
    let mut out = SchemaDiff::default();
    let tkey = |t: &TableDef| (name_key(&t.schema, opts), name_key(&t.name, opts));
    let targets: BTreeMap<(String, String), &TableDef> = target.tables.iter().map(|t| (tkey(t), t)).collect();
    let sources: BTreeMap<(String, String), &TableDef> = source.tables.iter().map(|t| (tkey(t), t)).collect();
    for (k, s) in &sources {
        let src_text = Some(table_sql(s, dialect, true));
        match targets.get(k) {
            None => out.differences.push(ObjectDiff {
                key: object_key(ObjectKind::Table, &s.schema, &s.name),
                kind: ObjectKind::Table,
                schema: s.schema.clone(),
                name: s.name.clone(),
                change: Change::Added,
                details: vec![format!("{} column(s)", s.columns.len())],
                source_text: src_text,
                target_text: None,
                warnings: vec![],
            }),
            Some(t) => {
                let (details, warnings) = compare_tables(s, t, opts);
                // a different column order alone is not worth a change
                if details.iter().all(|d| d.starts_with("column order")) {
                    out.identical += 1;
                    out.identical_objects
                        .push(ObjectRef::new(ObjectKind::Table, &s.schema, &s.name));
                    continue;
                }
                out.differences.push(ObjectDiff {
                    key: object_key(ObjectKind::Table, &s.schema, &s.name),
                    kind: ObjectKind::Table,
                    schema: s.schema.clone(),
                    name: s.name.clone(),
                    change: Change::Changed,
                    details,
                    source_text: src_text,
                    target_text: Some(table_sql(t, dialect, true)),
                    warnings,
                });
            }
        }
    }
    for (k, t) in &targets {
        if !sources.contains_key(k) {
            out.differences.push(ObjectDiff {
                key: object_key(ObjectKind::Table, &t.schema, &t.name),
                kind: ObjectKind::Table,
                schema: t.schema.clone(),
                name: t.name.clone(),
                change: Change::Removed,
                details: vec![format!("{} column(s)", t.columns.len())],
                source_text: None,
                target_text: Some(table_sql(t, dialect, true)),
                warnings: vec![format!("Table {} is dropped with all its data.", t.name)],
            });
        }
    }
    let mkey = |m: &ModuleDef| module_key(m, opts);
    let unreadable: HashSet<String> = source
        .unreadable
        .iter()
        .chain(&target.unreadable)
        .map(|n| name_key(n, opts))
        .collect();
    let readable = |m: &&ModuleDef| !unreadable.contains(&name_key(&format!("{}.{}", m.schema, m.name), opts));
    let tmods: BTreeMap<_, &ModuleDef> = target.modules.iter().filter(readable).map(|m| (mkey(m), m)).collect();
    let smods: BTreeMap<_, &ModuleDef> = source.modules.iter().filter(readable).map(|m| (mkey(m), m)).collect();
    let mut skipped: Vec<String> = source.unreadable.iter().chain(&target.unreadable).cloned().collect();
    skipped.sort_by_key(|s| s.to_lowercase());
    skipped.dedup_by_key(|s| s.to_lowercase());
    out.skipped = skipped;
    for (k, s) in &smods {
        let diff = |change, details: Vec<String>, target_text: Option<String>| ObjectDiff {
            key: object_key(s.kind, &s.schema, &s.name),
            kind: s.kind,
            schema: s.schema.clone(),
            name: s.name.clone(),
            change,
            details,
            source_text: Some(s.definition.clone()),
            target_text,
            warnings: vec![],
        };
        match tmods.get(k) {
            None => out.differences.push(diff(Change::Added, vec![], None)),
            Some(t) if norm_module(&s.definition, opts) != norm_module(&t.definition, opts) => {
                out.differences.push(diff(
                    Change::Changed,
                    vec!["definition differs".into()],
                    Some(t.definition.clone()),
                ))
            }
            Some(_) => {
                out.identical += 1;
                out.identical_objects.push(ObjectRef::new(s.kind, &s.schema, &s.name));
            }
        }
    }
    for (k, t) in &tmods {
        if !smods.contains_key(k) {
            out.differences.push(ObjectDiff {
                key: object_key(t.kind, &t.schema, &t.name),
                kind: t.kind,
                schema: t.schema.clone(),
                name: t.name.clone(),
                change: Change::Removed,
                details: vec![],
                source_text: None,
                target_text: Some(t.definition.clone()),
                warnings: vec![],
            });
        }
    }
    let order = |c: Change| match c {
        Change::Changed => 0,
        Change::Added => 1,
        Change::Removed => 2,
    };
    out.identical_objects
        .sort_by_key(|o| (kind_word(o.kind), o.schema.to_lowercase(), o.name.to_lowercase()));
    out.differences.sort_by(|a, b| {
        (
            order(a.change),
            kind_word(a.kind),
            a.schema.to_lowercase(),
            a.name.to_lowercase(),
        )
            .cmp(&(
                order(b.change),
                kind_word(b.kind),
                b.schema.to_lowercase(),
                b.name.to_lowercase(),
            ))
    });
    out
}

// ---------------------------------------------------------------------------
// synchronisation script
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncScript {
    pub script: String,
    pub warnings: Vec<String>,
    pub statements: usize,
}

/// A view, routine or trigger to drop or create, with its statement.
struct ModuleStep<'a> {
    rank: u8,
    module: &'a ModuleDef,
    statement: String,
}

impl<'a> ModuleStep<'a> {
    fn create(m: &'a ModuleDef, statement: String) -> Self {
        Self {
            rank: module_rank(m.kind),
            module: m,
            statement,
        }
    }

    fn drop(d: Dialect, m: &'a ModuleDef) -> Self {
        Self::create(m, drop_module(d, m))
    }
}

/// The statements of a sync script by phase, in the order they run.
#[derive(Default)]
struct Phases<'a> {
    create_schemas: Vec<String>,
    drop_fks: Vec<String>,
    /// Dropped in reverse dependency order.
    drop_modules: Vec<ModuleStep<'a>>,
    drop_indexes: Vec<String>,
    create_tables: Vec<String>,
    alter_tables: Vec<String>,
    drop_tables: Vec<String>,
    add_keys: Vec<String>,
    add_fks: Vec<String>,
    /// Created in dependency order.
    modules: Vec<ModuleStep<'a>>,
}

type ModuleKey = (&'static str, String, String);

/// What identifies a view, routine or trigger in both databases.
fn module_key(m: &ModuleDef, opts: &CompareOptions) -> ModuleKey {
    (kind_word(m.kind), name_key(&m.schema, opts), name_key(&m.name, opts))
}

/// The names SQL text mentions (identifier values, ASCII lower case).
fn mentioned_names(text: &str, d: Dialect) -> HashSet<String> {
    crate::lexer::tokenize(text, d)
        .iter()
        .filter(|t| t.is_name())
        .map(|t| t.ident_value().to_ascii_lowercase())
        .collect()
}

/// The statements of `steps` so that each comes after the modules it uses
/// (`dependents_first`: before them, for dropping). Rank breaks ties and
/// cycles: of the steps that wait for nothing, the first goes next.
fn dependency_order(mut steps: Vec<ModuleStep<'_>>, d: Dialect, dependents_first: bool) -> Vec<String> {
    if dependents_first {
        steps.sort_by_key(|s| std::cmp::Reverse(s.rank));
    } else {
        steps.sort_by_key(|s| s.rank);
    }
    let n = steps.len();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (j, s) in steps.iter().enumerate() {
        by_name.entry(s.module.name.to_ascii_lowercase()).or_default().push(j);
    }
    // waiting[k]: unplaced steps k waits for; frees[k]: steps that wait for k
    let mut waiting = vec![0usize; n];
    let mut frees: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, s) in steps.iter().enumerate() {
        for name in mentioned_names(&s.module.definition, d) {
            for &j in by_name.get(&name).into_iter().flatten().filter(|&&j| j != i) {
                // i uses j: j is created before i, and dropped after it
                let (first, then) = if dependents_first { (i, j) } else { (j, i) };
                waiting[then] += 1;
                frees[first].push(then);
            }
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..n).filter(|&k| waiting[k] == 0).map(Reverse).collect();
    let mut placed = vec![false; n];
    let mut first_left = 0;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = match std::iter::from_fn(|| ready.pop()).find(|Reverse(k)| !placed[*k]) {
            Some(Reverse(k)) => k,
            // a cycle: the first step left goes anyway
            None => {
                while placed[first_left] {
                    first_left += 1;
                }
                first_left
            }
        };
        placed[k] = true;
        out.push(std::mem::take(&mut steps[k].statement));
        for &m in &frees[k] {
            waiting[m] -= 1;
            if waiting[m] == 0 && !placed[m] {
                ready.push(Reverse(m));
            }
        }
    }
    out
}

fn drop_constraint(d: Dialect, t: &TableDef, name: &str, what: &str) -> String {
    let table = qualified(d, &t.schema, &t.name);
    match (d, what) {
        (Dialect::MySql, "fk") => format!("ALTER TABLE {table} DROP FOREIGN KEY {};", q(d, name)),
        (Dialect::MySql, "pk") => format!("ALTER TABLE {table} DROP PRIMARY KEY;"),
        (Dialect::MySql, "check") => format!("ALTER TABLE {table} DROP CHECK {};", q(d, name)),
        _ => format!("ALTER TABLE {table} DROP CONSTRAINT {};", q(d, name)),
    }
}

fn drop_index(d: Dialect, t: &TableDef, i: &IndexDef) -> String {
    if i.constraint && d != Dialect::MySql {
        return drop_constraint(d, t, &i.name, "unique");
    }
    match d {
        Dialect::Postgres => format!("DROP INDEX {};", qualified(d, &t.schema, &i.name)),
        _ => format!("DROP INDEX {} ON {};", q(d, &i.name), qualified(d, &t.schema, &t.name)),
    }
}

fn drop_module(d: Dialect, m: &ModuleDef) -> String {
    let what = match m.kind {
        ObjectKind::View => "VIEW",
        ObjectKind::Procedure => "PROCEDURE",
        ObjectKind::ScalarFunction | ObjectKind::TableFunction => "FUNCTION",
        ObjectKind::Trigger => "TRIGGER",
        _ => "VIEW",
    };
    if d == Dialect::Postgres && m.kind == ObjectKind::Trigger {
        let table = m.table.as_deref().unwrap_or("?");
        return format!(
            "DROP TRIGGER IF EXISTS {} ON {};",
            q(d, &m.name),
            qualified(d, &m.schema, table)
        );
    }
    // PostgreSQL functions: the name works when it is not overloaded
    format!("DROP {what} IF EXISTS {};", qualified(d, &m.schema, &m.name))
}

/// SQL Server: CREATE â†’ CREATE OR ALTER for views, routines and triggers.
fn create_or_alter(def: &str) -> String {
    let trimmed = def.trim_start();
    let lead = &def[..def.len() - trimmed.len()];
    // skip leading comments
    let mut rest = trimmed;
    let mut prefix = String::from(lead);
    loop {
        if rest.starts_with("--") {
            let end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
            prefix.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if rest.starts_with("/*") {
            let end = rest.find("*/").map(|i| i + 2).unwrap_or(rest.len());
            prefix.push_str(&rest[..end]);
            rest = &rest[end..];
        } else {
            let t = rest.trim_start();
            if t.len() == rest.len() {
                break;
            }
            prefix.push_str(&rest[..rest.len() - t.len()]);
            rest = t;
        }
    }
    if rest.len() >= 6
        && rest[..6].eq_ignore_ascii_case("CREATE")
        && !rest[6..].trim_start().to_ascii_uppercase().starts_with("OR ")
    {
        format!("{prefix}CREATE OR ALTER{}", &rest[6..])
    } else {
        def.to_string()
    }
}

fn module_rank(k: ObjectKind) -> u8 {
    match k {
        ObjectKind::ScalarFunction | ObjectKind::TableFunction => 1,
        ObjectKind::View => 2,
        ObjectKind::Procedure => 3,
        ObjectKind::Trigger => 4,
        _ => 5,
    }
}

/// A model's tables and modules by name (the first of a name wins).
struct ModelIndex<'a> {
    tables: HashMap<(String, String), &'a TableDef>,
    modules: HashMap<ModuleKey, &'a ModuleDef>,
}

impl<'a> ModelIndex<'a> {
    fn new(m: &'a SchemaModel, opts: &CompareOptions) -> Self {
        let mut tables = HashMap::new();
        for t in &m.tables {
            tables
                .entry((name_key(&t.schema, opts), name_key(&t.name, opts)))
                .or_insert(t);
        }
        let mut modules = HashMap::new();
        for x in &m.modules {
            modules.entry(module_key(x, opts)).or_insert(x);
        }
        Self { tables, modules }
    }
}

/// Columns of a changed table whose type (SQL Server: or NULL) changes.
/// SQL Server refuses ALTER COLUMN while an index, key, check or computed
/// column uses the column, PostgreSQL while a generated column does: those
/// are dropped and made again around the change.
struct AlteredColumns<'o> {
    names: HashSet<String>,
    /// Indexes, keys and checks are rebuilt too (SQL Server).
    rebuild: bool,
    d: Dialect,
    opts: &'o CompareOptions,
}

impl<'o> AlteredColumns<'o> {
    fn new(s: &TableDef, t: &TableDef, d: Dialect, opts: &'o CompareOptions) -> Self {
        let names = s
            .columns
            .iter()
            .filter_map(|c| {
                let o = named(&t.columns, &c.name, opts)?;
                (o.computed.is_none()
                    && c.computed.is_none()
                    && (norm_type(&c.data_type) != norm_type(&o.data_type)
                        || (d == Dialect::MsSql && c.nullable != o.nullable)))
                    .then(|| name_key(&c.name, opts))
            })
            .collect();
        Self {
            names,
            rebuild: d == Dialect::MsSql,
            d,
            opts,
        }
    }

    /// Whether an expression (filter, check, computed column) uses one.
    fn used_by(&self, text: &str) -> bool {
        !self.names.is_empty()
            && crate::lexer::tokenize(text, self.d)
                .iter()
                .any(|x| x.is_name() && self.names.contains(&name_key(&x.ident_value(), self.opts)))
    }

    /// Whether key or index columns (`col DESC` too) take one in, so the
    /// key or index is rebuilt.
    fn among(&self, cols: &[String]) -> bool {
        self.rebuild
            && cols.iter().any(|c| {
                self.names
                    .contains(&name_key(c.strip_suffix(" DESC").unwrap_or(c), self.opts))
            })
    }

    fn in_index(&self, i: &IndexDef) -> bool {
        self.among(&i.columns)
            || self.among(&i.include)
            || (self.rebuild && i.filter.as_deref().is_some_and(|f| self.used_by(f)))
    }

    fn in_check(&self, c: &CheckDef) -> bool {
        self.rebuild && self.used_by(&c.definition)
    }
}

/// Builds the statements of a sync script, one selected difference at a
/// time, then puts them in order.
struct SyncPlanner<'a> {
    d: Dialect,
    opts: &'a CompareOptions,
    target: &'a SchemaModel,
    sources: ModelIndex<'a>,
    targets: ModelIndex<'a>,
    /// Foreign keys of the target by the table they point at.
    pointing_at: HashMap<(String, String), Vec<(&'a TableDef, &'a ForeignKeyDef)>>,
    p: Phases<'a>,
    warnings: Vec<String>,
    /// Foreign keys already dropped: (schema, table, name).
    dropped_fks: HashSet<(String, String, String)>,
    /// PostgreSQL tables that lose a column or change a column's type.
    pg_changed_tables: Vec<String>,
}

impl<'a> SyncPlanner<'a> {
    fn new(source: &'a SchemaModel, target: &'a SchemaModel, d: Dialect, opts: &'a CompareOptions) -> Self {
        let mut pointing_at: HashMap<(String, String), Vec<(&TableDef, &ForeignKeyDef)>> = HashMap::new();
        for t in &target.tables {
            for f in &t.foreign_keys {
                pointing_at
                    .entry((name_key(&f.ref_schema, opts), name_key(&f.ref_table, opts)))
                    .or_default()
                    .push((t, f));
            }
        }
        Self {
            d,
            opts,
            target,
            sources: ModelIndex::new(source, opts),
            targets: ModelIndex::new(target, opts),
            pointing_at,
            p: Phases::default(),
            warnings: Vec::new(),
            dropped_fks: HashSet::new(),
            pg_changed_tables: Vec::new(),
        }
    }

    fn nk(&self, s: &str) -> String {
        name_key(s, self.opts)
    }

    fn table_key(&self, schema: &str, name: &str) -> (String, String) {
        (self.nk(schema), self.nk(name))
    }

    /// The foreign keys of the target that point at `t`.
    fn pointing_at(&self, t: &TableDef) -> Vec<(&'a TableDef, &'a ForeignKeyDef)> {
        self.pointing_at
            .get(&self.table_key(&t.schema, &t.name))
            .cloned()
            .unwrap_or_default()
    }

    fn plan(&mut self, o: &ObjectDiff) {
        self.warnings.extend(o.warnings.iter().cloned());
        let table_key = self.table_key(&o.schema, &o.name);
        let module_key = (kind_word(o.kind), self.nk(&o.schema), self.nk(&o.name));
        match (o.kind, o.change) {
            (ObjectKind::Table, Change::Added) => {
                if let Some(&s) = self.sources.tables.get(&table_key) {
                    self.add_table(s);
                }
            }
            (ObjectKind::Table, Change::Removed) => {
                if let Some(&t) = self.targets.tables.get(&table_key) {
                    self.remove_table(t);
                }
            }
            (ObjectKind::Table, Change::Changed) => {
                if let (Some(&s), Some(&t)) = (self.sources.tables.get(&table_key), self.targets.tables.get(&table_key))
                {
                    self.change_table(s, t);
                }
            }
            (_, Change::Removed) => {
                if let Some(&m) = self.targets.modules.get(&module_key) {
                    self.p.drop_modules.push(ModuleStep::drop(self.d, m));
                }
            }
            (_, Change::Added) => {
                if let Some(&m) = self.sources.modules.get(&module_key) {
                    self.p.modules.push(ModuleStep::create(m, m.definition.clone()));
                }
            }
            (_, Change::Changed) => {
                if let (Some(&m), Some(&old)) = (
                    self.sources.modules.get(&module_key),
                    self.targets.modules.get(&module_key),
                ) {
                    self.change_module(m, old);
                }
            }
        }
    }

    /// Whether two tables are the same table (a schema can hold one
    /// `Orders` and another schema its own).
    fn same_table(&self, a: &TableDef, b: &TableDef) -> bool {
        self.table_key(&a.schema, &a.name) == self.table_key(&b.schema, &b.name)
    }

    /// Two tables named in one message, with their schemas when they share
    /// a name.
    fn names_of(&self, a: &TableDef, b: &TableDef) -> (String, String) {
        if self.nk(&a.name) == self.nk(&b.name) && !self.same_table(a, b) {
            (format!("{}.{}", a.schema, a.name), format!("{}.{}", b.schema, b.name))
        } else {
            (a.name.clone(), b.name.clone())
        }
    }

    fn drop_fk(&mut self, t: &TableDef, f: &ForeignKeyDef) {
        if self
            .dropped_fks
            .insert((self.nk(&t.schema), self.nk(&t.name), self.nk(&f.name)))
        {
            self.p.drop_fks.push(drop_constraint(self.d, t, &f.name, "fk"));
        }
    }

    fn add_table(&mut self, s: &TableDef) {
        let d = self.d;
        self.p.create_tables.push(table_sql(s, d, false));
        for i in &s.indexes {
            self.p.add_keys.push(index_sql(s, i, d));
        }
        for c in &s.checks {
            self.p.add_keys.push(check_sql(s, c, d));
        }
        for f in &s.foreign_keys {
            self.p.add_fks.push(fk_sql(s, f, d));
        }
    }

    fn remove_table(&mut self, t: &TableDef) {
        // foreign keys of other tables that point at it go first
        for (other, f) in self.pointing_at(t) {
            if !self.same_table(other, t) {
                let (on, to) = self.names_of(other, t);
                self.warnings.push(format!(
                    "Foreign key {} on {on} references {to} and is dropped with it.",
                    f.name
                ));
            }
            self.drop_fk(other, f);
        }
        for f in &t.foreign_keys {
            self.drop_fk(t, f);
        }
        self.p
            .drop_tables
            .push(format!("DROP TABLE {};", qualified(self.d, &t.schema, &t.name)));
    }

    fn change_table(&mut self, s: &'a TableDef, t: &'a TableDef) {
        let opts = self.opts;
        let altered = AlteredColumns::new(s, t, self.d, opts);
        self.sync_foreign_keys(s, t, &altered);
        let pk_changed = s.primary_key.as_ref().map(|k| norm_cols(&k.columns))
            != t.primary_key.as_ref().map(|k| norm_cols(&k.columns))
            || t.primary_key.as_ref().is_some_and(|k| altered.among(&k.columns));
        if pk_changed {
            self.rebuild_primary_key(s, t);
        }
        self.sync_indexes_and_checks(s, t, &altered);
        self.sync_columns(s, t, &altered);
        if self.d == Dialect::Postgres
            && (t.columns.iter().any(|o| named(&s.columns, &o.name, opts).is_none())
                || s.columns.iter().any(|c| {
                    named(&t.columns, &c.name, opts).is_some_and(|o| norm_type(&c.data_type) != norm_type(&o.data_type))
                }))
        {
            self.pg_changed_tables.push(t.name.clone());
        }
    }

    /// Removed or changed foreign keys are dropped, new or changed ones added.
    fn sync_foreign_keys(&mut self, s: &TableDef, t: &TableDef, altered: &AlteredColumns<'_>) {
        let opts = self.opts;
        for f in &t.foreign_keys {
            let keep =
                named(&s.foreign_keys, &f.name, opts).is_some_and(|x| same_fk(x, f)) && !altered.among(&f.columns);
            if !keep {
                self.drop_fk(t, f);
            }
        }
        for f in &s.foreign_keys {
            let exists =
                named(&t.foreign_keys, &f.name, opts).is_some_and(|x| same_fk(x, f)) && !altered.among(&f.columns);
            if !exists {
                self.p.add_fks.push(fk_sql(s, f, self.d));
            }
        }
    }

    /// A changed primary key, and the foreign keys of other tables pointing
    /// at it, are dropped and made again.
    fn rebuild_primary_key(&mut self, s: &TableDef, t: &TableDef) {
        let d = self.d;
        for (other, f) in self.pointing_at(t) {
            if self.same_table(other, t) {
                continue;
            }
            self.drop_fk(other, f);
            let in_source = self
                .sources
                .tables
                .get(&self.table_key(&other.schema, &other.name))
                .and_then(|&o| Some((o, named(&o.foreign_keys, &f.name, self.opts)?)));
            match in_source {
                Some((o, sf)) => self.p.add_fks.push(fk_sql(o, sf, d)),
                None => {
                    let (on, to) = self.names_of(other, t);
                    self.warnings.push(format!(
                        "Foreign key {} on {on} depends on the changed primary key of {to} and is dropped.",
                        f.name
                    ));
                }
            }
        }
        if let Some(k) = &t.primary_key {
            self.p.drop_indexes.push(drop_constraint(d, t, &k.name, "pk"));
        }
        if let Some(pk) = pk_clause(s, d) {
            let table = qualified(d, &t.schema, &t.name);
            self.p.add_keys.push(format!("ALTER TABLE {table} ADD {pk};"));
        }
    }

    fn sync_indexes_and_checks(&mut self, s: &TableDef, t: &TableDef, altered: &AlteredColumns<'_>) {
        let (d, opts) = (self.d, self.opts);
        for i in &t.indexes {
            let keep = named(&s.indexes, &i.name, opts).is_some_and(|x| same_index(x, i)) && !altered.in_index(i);
            if !keep {
                self.p.drop_indexes.push(drop_index(d, t, i));
            }
        }
        for i in &s.indexes {
            let exists = named(&t.indexes, &i.name, opts).is_some_and(|x| same_index(x, i)) && !altered.in_index(i);
            if !exists {
                self.p.add_keys.push(index_sql(s, i, d));
            }
        }
        let same_check = |a: &CheckDef, b: &CheckDef| norm_check(&a.definition) == norm_check(&b.definition);
        for c in &t.checks {
            let keep = named(&s.checks, &c.name, opts).is_some_and(|x| same_check(x, c)) && !altered.in_check(c);
            if !keep {
                self.p.drop_indexes.push(drop_constraint(d, t, &c.name, "check"));
            }
        }
        for c in &s.checks {
            let exists = named(&t.checks, &c.name, opts).is_some_and(|x| same_check(x, c)) && !altered.in_check(c);
            if !exists {
                self.p.add_keys.push(check_sql(s, c, d));
            }
        }
    }

    fn sync_columns(&mut self, s: &TableDef, t: &TableDef, altered: &AlteredColumns<'_>) {
        let (d, opts) = (self.d, self.opts);
        let table = qualified(d, &t.schema, &t.name);
        let add_column = |c: &ColumnDef| match d {
            Dialect::MsSql => format!("ALTER TABLE {table} ADD {};", column_sql(c, d)),
            _ => format!("ALTER TABLE {table} ADD COLUMN {};", column_sql(c, d)),
        };
        // columns that go away first: a computed one may use a column that changes
        for o in &t.columns {
            if named(&s.columns, &o.name, opts).is_none() {
                if d == Dialect::MsSql
                    && let Some(n) = &o.default_name
                {
                    self.p
                        .alter_tables
                        .push(format!("ALTER TABLE {table} DROP CONSTRAINT {};", q(d, n)));
                }
                self.p
                    .alter_tables
                    .push(format!("ALTER TABLE {table} DROP COLUMN {};", q(d, &o.name)));
            }
        }
        // computed columns over a changing column are made again after it
        let mut readd = Vec::new();
        for c in &s.columns {
            if let (Some(expr), Some(o)) = (&c.computed, named(&t.columns, &c.name, opts))
                && o.computed.as_deref().map(|x| norm_text(x, opts)) == Some(norm_text(expr, opts))
                && altered.used_by(expr)
            {
                self.p
                    .alter_tables
                    .push(format!("ALTER TABLE {table} DROP COLUMN {};", q(d, &c.name)));
                readd.push(add_column(c));
            }
        }
        for c in &s.columns {
            match named(&t.columns, &c.name, opts) {
                None => self.p.alter_tables.push(add_column(c)),
                Some(o) => self.alter_column(&table, t, c, o),
            }
        }
        self.p.alter_tables.extend(readd);
    }

    /// Column `o` of the target becomes `c`.
    fn alter_column(&mut self, table: &str, t: &TableDef, c: &ColumnDef, o: &ColumnDef) {
        let (d, opts) = (self.d, self.opts);
        let col = q(d, &c.name);
        let type_changed = norm_type(&c.data_type) != norm_type(&o.data_type);
        let null_changed = c.nullable != o.nullable;
        let default_changed = norm_default(&c.default) != norm_default(&o.default);
        let computed_changed =
            c.computed.as_deref().map(|x| norm_text(x, opts)) != o.computed.as_deref().map(|x| norm_text(x, opts));
        // a computed column's type follows its expression
        if c.computed.is_some() && !computed_changed {
            return;
        }
        if c.identity != o.identity || computed_changed {
            self.p.alter_tables.push(format!(
                "-- {}.{}: {} differs; recreate the column by hand (Sqlotus does not rebuild columns).",
                t.name,
                c.name,
                if computed_changed {
                    "the computed expression"
                } else {
                    "identity"
                }
            ));
            if computed_changed {
                self.warnings.push(format!(
                    "Column {}.{}: the computed expression differs and must be changed by hand.",
                    t.name, c.name
                ));
            }
            return;
        }
        let alter = &mut self.p.alter_tables;
        match d {
            Dialect::MsSql => {
                if default_changed && let Some(n) = &o.default_name {
                    alter.push(format!("ALTER TABLE {table} DROP CONSTRAINT {};", q(d, n)));
                }
                if type_changed || null_changed {
                    alter.push(format!(
                        "ALTER TABLE {table} ALTER COLUMN {col} {} {};",
                        c.data_type,
                        if c.nullable { "NULL" } else { "NOT NULL" }
                    ));
                }
                if default_changed && let Some(def) = &c.default {
                    let name = c
                        .default_name
                        .as_ref()
                        .map(|n| format!("CONSTRAINT {} ", q(d, n)))
                        .unwrap_or_default();
                    alter.push(format!("ALTER TABLE {table} ADD {name}DEFAULT {def} FOR {col};"));
                }
            }
            Dialect::Postgres => {
                if type_changed {
                    alter.push(format!(
                        "ALTER TABLE {table} ALTER COLUMN {col} TYPE {} USING {col}::{};",
                        c.data_type, c.data_type
                    ));
                }
                if null_changed {
                    alter.push(format!(
                        "ALTER TABLE {table} ALTER COLUMN {col} {} NOT NULL;",
                        if c.nullable { "DROP" } else { "SET" }
                    ));
                }
                if default_changed {
                    alter.push(match &c.default {
                        Some(def) => format!("ALTER TABLE {table} ALTER COLUMN {col} SET DEFAULT {def};"),
                        None => format!("ALTER TABLE {table} ALTER COLUMN {col} DROP DEFAULT;"),
                    });
                }
            }
            Dialect::MySql => {
                if type_changed || null_changed || default_changed {
                    alter.push(format!("ALTER TABLE {table} MODIFY COLUMN {};", column_sql(c, d)));
                }
            }
        }
    }

    fn change_module(&mut self, m: &'a ModuleDef, old: &'a ModuleDef) {
        match self.d {
            Dialect::MsSql => self
                .p
                .modules
                .push(ModuleStep::create(m, create_or_alter(&m.definition))),
            // functions and procedures are CREATE OR REPLACE
            Dialect::Postgres if m.kind != ObjectKind::Trigger && m.kind != ObjectKind::View => {
                self.p.modules.push(ModuleStep::create(m, m.definition.clone()))
            }
            // a PostgreSQL view whose columns change cannot be replaced in place
            _ => {
                self.p.drop_modules.push(ModuleStep::drop(self.d, old));
                self.p.modules.push(ModuleStep::create(m, m.definition.clone()));
            }
        }
    }

    /// PostgreSQL will not drop a view, or change a column, that other views
    /// (or triggers) are built on: those go first and come back afterwards.
    fn rebuild_postgres_dependents(&mut self) {
        let (d, opts, target) = (self.d, self.opts, self.target);
        // the target modules that name each name
        let mut users: HashMap<String, Vec<usize>> = HashMap::new();
        for (k, m) in target.modules.iter().enumerate() {
            for name in mentioned_names(&m.definition, d) {
                users.entry(name).or_default().push(k);
            }
        }
        let users_of = |name: &str| users.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
        let mut dropped: HashSet<ModuleKey> = self.p.drop_modules.iter().map(|s| module_key(s.module, opts)).collect();
        let mut created: HashSet<ModuleKey> = self.p.modules.iter().map(|s| module_key(s.module, opts)).collect();
        let mut queue: Vec<&'a ModuleDef> = self.p.drop_modules.iter().map(|s| s.module).collect();
        let mut over_tables: Vec<usize> = self
            .pg_changed_tables
            .iter()
            .flat_map(|t| users_of(t))
            .filter(|&k| target.modules[k].kind == ObjectKind::View)
            .collect();
        over_tables.sort_unstable();
        over_tables.dedup();
        let mut rebuild = |m: &'a ModuleDef, this: &mut Self, queue: &mut Vec<&'a ModuleDef>| {
            if !dropped.insert(module_key(m, opts)) {
                return;
            }
            this.p.drop_modules.push(ModuleStep::drop(d, m));
            match this.sources.modules.get(&module_key(m, opts)) {
                Some(&s) => {
                    if created.insert(module_key(s, opts)) {
                        this.p.modules.push(ModuleStep::create(s, s.definition.clone()));
                    }
                }
                None => this.warnings.push(format!(
                    "{} {} is built on an object that changes and is dropped with it (it is not in the source).",
                    kind_word(m.kind),
                    m.name
                )),
            }
            queue.push(m);
        };
        for k in over_tables {
            rebuild(&target.modules[k], self, &mut queue);
        }
        while let Some(gone) = queue.pop() {
            let key = module_key(gone, opts);
            for k in users_of(&gone.name) {
                let m = &target.modules[k];
                if matches!(m.kind, ObjectKind::View | ObjectKind::Trigger) && module_key(m, opts) != key {
                    rebuild(m, self, &mut queue);
                }
            }
        }
    }

    /// Schemas that new objects need (not MySQL, where a schema is a database).
    fn create_schemas(&mut self, chosen: &[&ObjectDiff]) {
        let d = self.d;
        if d == Dialect::MySql {
            return;
        }
        let existing: HashSet<String> = self
            .target
            .tables
            .iter()
            .map(|t| self.nk(&t.schema))
            .chain(self.target.modules.iter().map(|m| self.nk(&m.schema)))
            .collect();
        let mut needed: Vec<String> = Vec::new();
        for o in chosen {
            let builtin = matches!(o.schema.to_lowercase().as_str(), "dbo" | "public");
            if o.change == Change::Added
                && !o.schema.is_empty()
                && !builtin
                && !existing.contains(&self.nk(&o.schema))
                && !needed.iter().any(|s| self.nk(s) == self.nk(&o.schema))
            {
                needed.push(o.schema.clone());
            }
        }
        for s in needed {
            self.p.create_schemas.push(match d {
                Dialect::MsSql => {
                    let lit = |x: &str| x.replace('\'', "''");
                    format!(
                        "IF SCHEMA_ID(N'{}') IS NULL EXEC(N'CREATE SCHEMA {}');",
                        lit(&s),
                        lit(&q(d, &s))
                    )
                }
                _ => format!("CREATE SCHEMA IF NOT EXISTS {};", q(d, &s)),
            });
        }
    }

    /// The script: the phases in order, each statement once, with the
    /// warnings at the top.
    fn into_script(mut self) -> SyncScript {
        let d = self.d;
        let drop_modules = dependency_order(std::mem::take(&mut self.p.drop_modules), d, true);
        let modules = dependency_order(std::mem::take(&mut self.p.modules), d, false);
        let p = &self.p;
        let mut warnings = self.warnings;
        let mut seen = HashSet::new();
        warnings.retain(|w| seen.insert(w.clone()));
        let mut script = String::new();
        let mut statements = 0usize;
        let go = if d == Dialect::MsSql { "GO\n" } else { "" };
        script.push_str("-- Schema synchronisation script generated by Sqlotus.\n-- Review it before running: it changes the target database.\n");
        if !warnings.is_empty() {
            script.push_str("--\n-- WARNINGS:\n");
            for w in &warnings {
                script.push_str(&format!("--   * {w}\n"));
            }
        }
        let mut section = |title: &str, items: &[String], script: &mut String| {
            if items.is_empty() {
                return;
            }
            script.push_str(&format!("\n-- {title}\n"));
            for i in items {
                script.push_str(i);
                script.push('\n');
                if !i.starts_with("--") {
                    statements += 1;
                }
            }
            script.push_str(go);
        };
        section("Create schemas", &p.create_schemas, &mut script);
        section("Drop foreign keys", &p.drop_fks, &mut script);
        section("Drop views, routines and triggers", &drop_modules, &mut script);
        section("Drop indexes, keys and checks", &p.drop_indexes, &mut script);
        section("Create tables", &p.create_tables, &mut script);
        section("Change tables", &p.alter_tables, &mut script);
        section("Drop tables", &p.drop_tables, &mut script);
        section("Create keys, indexes and checks", &p.add_keys, &mut script);
        section("Create foreign keys", &p.add_fks, &mut script);
        if !modules.is_empty() {
            // the comment gets its own batch: SQL Server would store it as part of the first module
            script.push_str("\n-- Create or change views, routines and triggers\n");
            script.push_str(go);
            for m in &modules {
                let m = m.trim_end();
                script.push_str(m);
                if d != Dialect::MsSql && !m.ends_with(';') {
                    script.push(';');
                }
                script.push('\n');
                script.push_str(go);
                statements += 1;
            }
        }
        if statements == 0 {
            script.push_str("\n-- Nothing to change for the selected objects.\n");
        }
        SyncScript {
            script,
            warnings,
            statements,
        }
    }
}

/// A script that applies the selected differences (by key) to the target.
pub fn sync_script(
    source: &SchemaModel,
    target: &SchemaModel,
    diff: &SchemaDiff,
    selected: &[String],
    dialect: Dialect,
    opts: &CompareOptions,
) -> SyncScript {
    let pick: HashSet<&str> = selected.iter().map(String::as_str).collect();
    let chosen: Vec<&ObjectDiff> = diff
        .differences
        .iter()
        .filter(|x| pick.contains(x.key.as_str()))
        .collect();
    let mut planner = SyncPlanner::new(source, target, dialect, opts);
    for o in &chosen {
        planner.plan(o);
    }
    if dialect == Dialect::Postgres {
        planner.rebuild_postgres_dependents();
    }
    planner.create_schemas(&chosen);
    planner.into_script()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            ..Default::default()
        }
    }

    fn customers() -> TableDef {
        TableDef {
            schema: "dbo".into(),
            name: "Customers".into(),
            columns: vec![
                ColumnDef {
                    identity: true,
                    ..col("Id", "int", false)
                },
                col("Name", "nvarchar(100)", false),
                ColumnDef {
                    default: Some("('VN')".into()),
                    default_name: Some("DF_Customers_Country".into()),
                    ..col("Country", "char(2)", true)
                },
            ],
            primary_key: Some(KeyDef {
                name: "PK_Customers".into(),
                columns: vec!["Id".into()],
            }),
            indexes: vec![IndexDef {
                name: "IX_Customers_Name".into(),
                columns: vec!["Name".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn orders() -> TableDef {
        TableDef {
            schema: "dbo".into(),
            name: "Orders".into(),
            columns: vec![
                ColumnDef {
                    identity: true,
                    ..col("Id", "int", false)
                },
                col("CustomerId", "int", false),
                col("Total", "decimal(12, 2)", false),
            ],
            primary_key: Some(KeyDef {
                name: "PK_Orders".into(),
                columns: vec!["Id".into()],
            }),
            foreign_keys: vec![ForeignKeyDef {
                name: "FK_Orders_Customers".into(),
                columns: vec!["CustomerId".into()],
                ref_schema: "dbo".into(),
                ref_table: "Customers".into(),
                ref_columns: vec!["Id".into()],
                ..Default::default()
            }],
            checks: vec![CheckDef {
                name: "CK_Orders_Total".into(),
                definition: "([Total]>=(0))".into(),
            }],
            ..Default::default()
        }
    }

    fn view(def: &str) -> ModuleDef {
        ModuleDef {
            schema: "dbo".into(),
            name: "vTotals".into(),
            kind: ObjectKind::View,
            definition: def.into(),
            table: None,
        }
    }

    fn model() -> SchemaModel {
        SchemaModel {
            unreadable: vec![],
            tables: vec![customers(), orders()],
            modules: vec![view(
                "CREATE VIEW dbo.vTotals AS\nSELECT CustomerId, SUM(Total) AS Total FROM dbo.Orders GROUP BY CustomerId",
            )],
        }
    }

    const MS: Dialect = Dialect::MsSql;

    #[test]
    fn identical_and_empty() {
        let opts = CompareOptions::default();
        let d = compare(&model(), &model(), MS, &opts);
        assert!(d.differences.is_empty());
        assert_eq!(d.identical, 3);
        // the identical objects are listed, sorted, with the same keys as differences
        assert_eq!(d.identical_objects.len(), 3);
        let keys: Vec<&str> = d.identical_objects.iter().map(|o| o.key.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert!(
            keys.iter().all(|k| k.contains(':') && k.to_lowercase() == **k),
            "{keys:?}"
        );
        assert_eq!(keys, sorted);
        let empty = compare(&SchemaModel::default(), &SchemaModel::default(), MS, &opts);
        assert!(empty.differences.is_empty() && empty.identical == 0);
        let s = sync_script(&model(), &model(), &d, &[], MS, &opts);
        assert!(s.script.contains("Nothing to change") && s.statements == 0);
    }

    #[test]
    fn whitespace_case_and_default_parentheses_are_not_differences() {
        let mut t = model();
        t.modules[0].definition = "create view dbo.vTotals as select CustomerId,   SUM(Total) AS Total\r\n   FROM dbo.Orders GROUP BY CustomerId;".into();
        t.tables[0].columns[2].default = Some("'VN'".into());
        t.tables[0].name = "CUSTOMERS".into();
        t.tables[0].columns[1].data_type = "NVARCHAR(100)".into();
        let ws = CompareOptions {
            ignore_case: true,
            ..Default::default()
        };
        assert!(compare(&model(), &t, MS, &ws).differences.is_empty());
        // without ignoring case the view text differs
        let strict = CompareOptions {
            ignore_case: false,
            ..Default::default()
        };
        let d = compare(&model(), &t, MS, &strict);
        assert_eq!(d.differences.len(), 1);
        assert_eq!(d.differences[0].kind, ObjectKind::View);
        // case-sensitive names (Postgres): CUSTOMERS is another table
        let pg = CompareOptions {
            case_insensitive_names: false,
            ignore_case: true,
            ..Default::default()
        };
        let d = compare(&model(), &t, Dialect::Postgres, &pg);
        assert_eq!(d.differences.iter().filter(|x| x.kind == ObjectKind::Table).count(), 2);
    }

    #[test]
    fn table_changes_and_warnings() {
        let src = model();
        let mut tgt = model();
        let c = &mut tgt.tables[0];
        c.columns[1].data_type = "nvarchar(200)".into(); // source is shorter â†’ narrowing
        c.columns[2].nullable = false; // source allows NULL
        c.columns.push(col("Legacy", "int", true)); // dropped
        c.indexes.clear(); // index added
        tgt.tables[1].columns.retain(|x| x.name != "Total");
        tgt.tables[1].checks.clear();
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let cust = d.differences.iter().find(|x| x.name == "Customers").unwrap();
        assert!(
            cust.details
                .contains(&"column Name: type nvarchar(200) â†’ nvarchar(100)".to_string()),
            "{:?}",
            cust.details
        );
        assert!(cust.details.contains(&"column Legacy removed".to_string()));
        assert!(cust.details.contains(&"index IX_Customers_Name added".to_string()));
        assert!(cust.warnings.iter().any(|w| w.contains("shorter")));
        assert!(cust.warnings.iter().any(|w| w.contains("Legacy is dropped")));
        let ord = d.differences.iter().find(|x| x.name == "Orders").unwrap();
        assert!(ord.warnings.iter().any(|w| w.contains("NOT NULL without a default")));
        let keys: Vec<String> = d.differences.iter().map(|x| x.key.clone()).collect();
        let s = sync_script(&src, &tgt, &d, &keys, MS, &opts);
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Customers ALTER COLUMN Name nvarchar(100) NOT NULL;"),
            "{}",
            s.script
        );
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Customers ALTER COLUMN Country char(2) NULL;")
        );
        assert!(s.script.contains("ALTER TABLE dbo.Customers DROP COLUMN Legacy;"));
        assert!(
            s.script
                .contains("CREATE NONCLUSTERED INDEX IX_Customers_Name ON dbo.Customers (Name);")
        );
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Orders ADD Total decimal(12, 2) NOT NULL;")
        );
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Orders ADD CONSTRAINT CK_Orders_Total CHECK ([Total]>=(0));")
        );
        assert!(s.script.contains("-- WARNINGS:"));
        // the add-column phase comes before the check that uses it
        assert!(s.script.find("ADD Total").unwrap() < s.script.find("CK_Orders_Total CHECK").unwrap());
    }

    #[test]
    fn adding_and_dropping_tables_in_dependency_order() {
        let src = model();
        let tgt = SchemaModel {
            tables: vec![],
            modules: vec![],
            ..Default::default()
        };
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let keys: Vec<String> = d.differences.iter().map(|x| x.key.clone()).collect();
        let s = sync_script(&src, &tgt, &d, &keys, MS, &opts);
        let create_c = s.script.find("CREATE TABLE dbo.Customers").unwrap();
        let create_o = s.script.find("CREATE TABLE dbo.Orders").unwrap();
        let fk = s.script.find("ADD CONSTRAINT FK_Orders_Customers").unwrap();
        let view = s.script.find("CREATE VIEW dbo.vTotals").unwrap();
        assert!(create_c < fk && create_o < fk && fk < view, "{}", s.script);
        assert!(s.script.contains("Id int IDENTITY(1, 1) NOT NULL"));
        assert!(s.script.contains("CONSTRAINT DF_Customers_Country DEFAULT ('VN')"));
        assert!(s.script.contains(
            "CREATE VIEW dbo.vTotals AS\nSELECT CustomerId, SUM(Total) AS Total FROM dbo.Orders GROUP BY CustomerId\nGO"
        ));

        // the reverse: dropping Customers alone also drops the foreign key pointing at it
        let d = compare(&tgt, &src, MS, &opts);
        let only_customers = vec!["table:dbo.customers".to_string()];
        let s = sync_script(&tgt, &src, &d, &only_customers, MS, &opts);
        assert!(
            s.script.find("DROP CONSTRAINT FK_Orders_Customers").unwrap()
                < s.script.find("DROP TABLE dbo.Customers").unwrap()
        );
        assert!(
            s.warnings
                .iter()
                .any(|w| w.contains("FK_Orders_Customers on Orders references Customers"))
        );
        assert!(s.warnings.iter().any(|w| w.contains("dropped with all its data")));
    }

    #[test]
    fn primary_key_change_rebuilds_dependent_foreign_keys() {
        let src = model();
        let mut tgt = model();
        tgt.tables[0].primary_key = Some(KeyDef {
            name: "PK_Old".into(),
            columns: vec!["Name".into()],
        });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], MS, &opts);
        let drop_fk = s
            .script
            .find("ALTER TABLE dbo.Orders DROP CONSTRAINT FK_Orders_Customers;")
            .unwrap();
        let drop_pk = s.script.find("DROP CONSTRAINT PK_Old").unwrap();
        let add_pk = s.script.find("ADD CONSTRAINT PK_Customers PRIMARY KEY (Id)").unwrap();
        let add_fk = s
            .script
            .find("ALTER TABLE dbo.Orders ADD CONSTRAINT FK_Orders_Customers")
            .unwrap();
        assert!(drop_fk < drop_pk && drop_pk < add_pk && add_pk < add_fk, "{}", s.script);
    }

    #[test]
    fn modules_per_dialect() {
        let src = model();
        let mut tgt = model();
        tgt.modules[0].definition = "CREATE VIEW dbo.vTotals AS SELECT 1 AS x".into();
        tgt.modules.push(ModuleDef {
            schema: "dbo".into(),
            name: "pOld".into(),
            kind: ObjectKind::Procedure,
            definition: "CREATE PROCEDURE dbo.pOld AS SELECT 1".into(),
            table: None,
        });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let keys: Vec<String> = d.differences.iter().map(|x| x.key.clone()).collect();
        let s = sync_script(&src, &tgt, &d, &keys, MS, &opts);
        assert!(s.script.contains("CREATE OR ALTER VIEW dbo.vTotals AS"), "{}", s.script);
        assert!(s.script.contains("DROP PROCEDURE IF EXISTS dbo.pOld;"));
        let s = sync_script(&src, &tgt, &d, &keys, Dialect::MySql, &opts);
        assert!(s.script.find("DROP VIEW IF EXISTS").unwrap() < s.script.find("CREATE VIEW dbo.vTotals").unwrap());
        assert_eq!(
            create_or_alter("-- note\nCREATE   PROCEDURE p AS SELECT 1"),
            "-- note\nCREATE OR ALTER   PROCEDURE p AS SELECT 1"
        );
        assert_eq!(
            create_or_alter("CREATE OR ALTER VIEW v AS SELECT 1"),
            "CREATE OR ALTER VIEW v AS SELECT 1"
        );
        let o = CompareOptions::default();
        assert_eq!(
            norm_module("CREATE OR ALTER VIEW v AS SELECT 1", &o),
            norm_module(
                "create   view v AS SELECT 1",
                &CompareOptions {
                    ignore_case: false,
                    ..o
                }
            )
            .replace("create", "CREATE")
            .replace("view", "VIEW")
        );
        assert_eq!(
            norm_module("ALTER PROCEDURE p AS SELECT 1", &o),
            norm_module("CREATE PROCEDURE p AS SELECT 1", &o)
        );
        assert_eq!(
            norm_module("CREATE OR REPLACE VIEW v AS SELECT 1", &o),
            "CREATE VIEW v AS SELECT 1"
        );
    }

    #[test]
    fn reserved_and_unusual_names_are_quoted() {
        let t = TableDef {
            schema: "dbo".into(),
            name: "Order Lines".into(),
            columns: vec![col("LineNo", "int", false), col("User", "nvarchar(10)", true)],
            primary_key: Some(KeyDef {
                name: "PK_Lines".into(),
                columns: vec!["LineNo".into()],
            }),
            ..Default::default()
        };
        let src = SchemaModel {
            tables: vec![t],
            modules: vec![],
            ..Default::default()
        };
        let d = compare(&src, &SchemaModel::default(), MS, &CompareOptions::default());
        let s = sync_script(
            &src,
            &SchemaModel::default(),
            &d,
            &[d.differences[0].key.clone()],
            MS,
            &CompareOptions::default(),
        );
        assert!(
            !s.script.contains("CREATE SCHEMA"),
            "dbo exists when the target has objects in it"
        );
        let sales = TableDef {
            schema: "sales".into(),
            name: "Regions".into(),
            columns: vec![col("Code", "char(2)", false)],
            ..Default::default()
        };
        let with_sales = SchemaModel {
            tables: vec![sales],
            modules: vec![],
            ..Default::default()
        };
        let target = SchemaModel {
            tables: vec![customers()],
            modules: vec![],
            ..Default::default()
        };
        let d2 = compare(&with_sales, &target, MS, &CompareOptions::default());
        let added: Vec<String> = d2
            .differences
            .iter()
            .filter(|x| x.change == Change::Added)
            .map(|x| x.key.clone())
            .collect();
        let s2 = sync_script(&with_sales, &target, &d2, &added, MS, &CompareOptions::default());
        assert!(
            s2.script
                .find("IF SCHEMA_ID(N'sales') IS NULL EXEC(N'CREATE SCHEMA sales');")
                .unwrap()
                < s2.script.find("CREATE TABLE sales.Regions").unwrap()
        );
        let s3 = sync_script(
            &with_sales,
            &target,
            &d2,
            &added,
            Dialect::Postgres,
            &CompareOptions::default(),
        );
        assert!(s3.script.contains("CREATE SCHEMA IF NOT EXISTS sales;"));
        assert!(s.script.contains("CREATE TABLE dbo.[Order Lines] (\n    [LineNo] int NOT NULL,\n    [User] nvarchar(10) NULL,\n    CONSTRAINT PK_Lines PRIMARY KEY ([LineNo])"), "{}", s.script);
    }

    #[test]
    fn unique_constraints_and_expression_indexes() {
        let uq = IndexDef {
            name: "UQ_Name".into(),
            columns: vec!["Name".into()],
            unique: true,
            constraint: true,
            ..Default::default()
        };
        let mut src = model();
        src.tables[0].indexes.push(uq.clone());
        let mut tgt = model();
        tgt.tables[0].indexes.push(IndexDef {
            columns: vec!["Name".into(), "Country".into()],
            ..uq
        });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], MS, &opts);
        assert!(
            s.script.contains("ALTER TABLE dbo.Customers DROP CONSTRAINT UQ_Name;"),
            "{}",
            s.script
        );
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Customers ADD CONSTRAINT UQ_Name UNIQUE (Name);")
        );
        let a = IndexDef {
            name: "ix".into(),
            definition: Some("CREATE INDEX ix ON public.t USING btree (lower(name))".into()),
            ..Default::default()
        };
        let b = IndexDef {
            definition: Some("CREATE INDEX ix ON public.t USING btree (upper(name))".into()),
            ..a.clone()
        };
        assert!(same_index(&a, &a.clone()) && !same_index(&a, &b));
    }

    #[test]
    fn unreadable_definitions_are_skipped_not_dropped() {
        // the source login cannot read the view: it must not look removed
        let src = SchemaModel {
            modules: vec![],
            unreadable: vec!["dbo.vTotals".into()],
            ..model()
        };
        let d = compare(&src, &model(), MS, &CompareOptions::default());
        assert!(
            d.differences.iter().all(|x| x.kind != ObjectKind::View),
            "{:?}",
            d.differences
        );
        assert_eq!(d.skipped, vec!["dbo.vTotals".to_string()]);
    }

    #[test]
    fn narrowing_rules() {
        assert!(narrowing("varchar(50)", "varchar(20)").is_some());
        assert!(narrowing("varchar(20)", "varchar(50)").is_none());
        assert!(narrowing("varchar(max)", "varchar(50)").is_some());
        assert!(narrowing("bigint", "int").is_some());
        assert!(narrowing("int", "bigint").is_none());
        assert!(narrowing("decimal(12, 4)", "decimal(12, 2)").is_some());
        assert!(narrowing("int", "nvarchar(10)").is_some());
        assert_eq!(norm_default(&Some("((0))".into())), Some("0".into()));
        assert_eq!(norm_default(&Some("(getdate())".into())), Some("getdate()".into()));
        assert_eq!(norm_default(&Some("(1)+(2)".into())), Some("(1)+(2)".into()));
    }

    #[test]
    fn postgres_and_mysql_ddl() {
        let src = model();
        let mut tgt = model();
        tgt.tables[0].columns[1].data_type = "nvarchar(50)".into();
        tgt.tables[0].columns[2].default = None;
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, Dialect::Postgres, &opts);
        let s = sync_script(
            &src,
            &tgt,
            &d,
            &["table:dbo.customers".into()],
            Dialect::Postgres,
            &opts,
        );
        assert!(
            s.script.contains(
                "ALTER TABLE dbo.\"Customers\" ALTER COLUMN \"Name\" TYPE nvarchar(100) USING \"Name\"::nvarchar(100);"
            ),
            "{}",
            s.script
        );
        assert!(s.script.contains("ALTER COLUMN \"Country\" SET DEFAULT ('VN');"));
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], Dialect::MySql, &opts);
        assert!(
            s.script
                .contains("ALTER TABLE dbo.Customers MODIFY COLUMN Name nvarchar(100) NOT NULL;"),
            "{}",
            s.script
        );
        assert!(!s.script.contains("GO"));
    }

    fn sync_all(src: &SchemaModel, tgt: &SchemaModel, d: Dialect) -> String {
        let opts = CompareOptions::default();
        let diff = compare(src, tgt, d, &opts);
        let keys: Vec<String> = diff.differences.iter().map(|x| x.key.clone()).collect();
        sync_script(src, tgt, &diff, &keys, d, &opts).script
    }

    fn module(kind: ObjectKind, name: &str, def: &str) -> ModuleDef {
        ModuleDef {
            schema: "public".into(),
            name: name.into(),
            kind,
            definition: def.into(),
            table: None,
        }
    }

    #[test]
    fn views_go_in_dependency_order() {
        let pg = Dialect::Postgres;
        let base = module(
            ObjectKind::View,
            "v_base",
            "CREATE VIEW public.v_base AS SELECT id FROM t",
        );
        let top = module(
            ObjectKind::View,
            "v_top",
            "CREATE VIEW public.v_top AS SELECT id FROM public.v_base",
        );
        let both = SchemaModel {
            modules: vec![top.clone(), base.clone()],
            ..Default::default()
        };
        let none = SchemaModel::default();
        let at = |s: &str, x: &str| s.find(x).unwrap_or_else(|| panic!("{x} in {s}"));
        // created: the view it uses first; dropped: the view using it first
        let up = sync_all(&both, &none, pg);
        assert!(at(&up, "VIEW public.v_base") < at(&up, "VIEW public.v_top"), "{up}");
        let down = sync_all(&none, &both, pg);
        assert!(
            at(&down, "DROP VIEW IF EXISTS public.v_top") < at(&down, "DROP VIEW IF EXISTS public.v_base"),
            "{down}"
        );
        // a changed view takes the unchanged views built on it along (PostgreSQL)
        let changed = SchemaModel {
            modules: vec![
                top.clone(),
                module(
                    ObjectKind::View,
                    "v_base",
                    "CREATE VIEW public.v_base AS SELECT id, name FROM t",
                ),
            ],
            ..Default::default()
        };
        let script = sync_all(&changed, &both, pg);
        assert!(
            at(&script, "DROP VIEW IF EXISTS public.v_top") < at(&script, "DROP VIEW IF EXISTS public.v_base"),
            "{script}"
        );
        assert!(
            at(&script, "CREATE VIEW public.v_base") < at(&script, "CREATE VIEW public.v_top"),
            "{script}"
        );
    }

    #[test]
    fn changed_column_types_rebuild_what_uses_them() {
        let ms = Dialect::MsSql;
        let table = |amount: &str| TableDef {
            schema: "dbo".into(),
            name: "Kept".into(),
            columns: vec![
                col("Id", "int", false),
                col("Amount", amount, false),
                ColumnDef {
                    computed: Some("([Amount]*(2))".into()),
                    ..col("Twice", "decimal(13, 2)", true)
                },
                col("Gone", "int", true),
            ],
            primary_key: Some(KeyDef {
                name: "PK_Kept".into(),
                columns: vec!["Id".into()],
            }),
            indexes: vec![IndexDef {
                name: "IX_Kept_Amount".into(),
                columns: vec!["Amount".into()],
                ..Default::default()
            }],
            checks: vec![CheckDef {
                name: "CK_Kept_Amount".into(),
                definition: "([Amount]>=(0))".into(),
            }],
            ..Default::default()
        };
        let mut src = table("decimal(12, 2)");
        src.columns.retain(|c| c.name != "Gone");
        let src = SchemaModel {
            tables: vec![src],
            ..Default::default()
        };
        let tgt = SchemaModel {
            tables: vec![table("decimal(9, 2)")],
            ..Default::default()
        };
        let script = sync_all(&src, &tgt, ms);
        let at = |x: &str| script.find(x).unwrap_or_else(|| panic!("{x} in {script}"));
        // the index, the check and the computed column go before the change and come back after
        assert!(at("DROP INDEX IX_Kept_Amount") < at("ALTER COLUMN Amount"));
        assert!(at("DROP CONSTRAINT CK_Kept_Amount") < at("ALTER COLUMN Amount"));
        assert!(at("DROP COLUMN Twice") < at("ALTER COLUMN Amount"));
        assert!(at("ALTER COLUMN Amount") < at("ADD Twice AS"));
        assert!(at("ALTER COLUMN Amount") < at("NONCLUSTERED INDEX IX_Kept_Amount ON"));
        assert!(at("ALTER COLUMN Amount") < at("ADD CONSTRAINT CK_Kept_Amount"));
        // a dropped column goes first; a computed column's type is not altered
        assert!(at("DROP COLUMN Gone") < at("ALTER COLUMN Amount"));
        assert!(!script.contains("ALTER COLUMN Twice"), "{script}");
        // PostgreSQL: only the generated column has to make way
        let pg = sync_all(&src, &tgt, Dialect::Postgres);
        assert!(
            pg.contains("DROP COLUMN \"Twice\"") && pg.contains("ADD COLUMN \"Twice\""),
            "{pg}"
        );
        assert!(!pg.contains("DROP INDEX"), "{pg}");
    }

    #[test]
    fn tables_of_one_name_in_two_schemas() {
        let in_schema = |schema: &str, refs: &str| TableDef {
            schema: schema.into(),
            name: "Orders".into(),
            columns: vec![col("Id", "int", false), col("Ref", "int", true)],
            primary_key: Some(KeyDef {
                name: format!("PK_{schema}_Orders"),
                columns: vec!["Id".into()],
            }),
            foreign_keys: vec![ForeignKeyDef {
                name: "FK_Orders_Ref".into(),
                columns: vec!["Ref".into()],
                ref_schema: refs.into(),
                ref_table: "Orders".into(),
                ref_columns: vec!["Id".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let both = SchemaModel {
            tables: vec![in_schema("dbo", "sales"), in_schema("sales", "dbo")],
            ..Default::default()
        };
        // each table's foreign key is dropped, though both have the same name
        let down = sync_all(&SchemaModel::default(), &both, MS);
        for drop in [
            "ALTER TABLE dbo.Orders DROP CONSTRAINT FK_Orders_Ref;",
            "ALTER TABLE sales.Orders DROP CONSTRAINT FK_Orders_Ref;",
        ] {
            assert!(
                down.contains(drop),
                "{drop}
{down}"
            );
        }
        assert!(down.contains("on sales.Orders references dbo.Orders"), "{down}");
        // a new primary key on dbo.Orders: the key of sales.Orders pointing at it goes and comes back
        let mut changed = both.clone();
        changed.tables[0].primary_key = Some(KeyDef {
            name: "PK_dbo_Orders".into(),
            columns: vec!["Id".into(), "Ref".into()],
        });
        let script = sync_all(&changed, &both, MS);
        let at = |x: &str| script.find(x).unwrap_or_else(|| panic!("{x} in {script}"));
        assert!(
            at("ALTER TABLE sales.Orders DROP CONSTRAINT FK_Orders_Ref;")
                < at("ALTER TABLE dbo.Orders DROP CONSTRAINT PK_dbo_Orders;")
        );
        assert!(at("ADD CONSTRAINT PK_dbo_Orders") < at("ALTER TABLE sales.Orders ADD CONSTRAINT FK_Orders_Ref"));
    }
}
