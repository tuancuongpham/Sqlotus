//! Schema compare: the differences between two databases (tables, columns,
//! keys, indexes, foreign keys, checks, views, routines, triggers) and a
//! script that makes the target match the source. The script is for review:
//! operations that can lose data are listed as warnings at the top.

use std::collections::{BTreeMap, HashSet};

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDiff {
    pub differences: Vec<ObjectDiff>,
    /// Objects present in both and identical.
    pub identical: usize,
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
        Self { ignore_whitespace: true, ignore_case: false, case_insensitive_names: true }
    }
}

// ---------------------------------------------------------------------------
// normalisation
// ---------------------------------------------------------------------------

fn name_key(s: &str, opts: &CompareOptions) -> String {
    if opts.case_insensitive_names { s.to_lowercase() } else { s.to_string() }
}

fn norm_type(t: &str) -> String {
    t.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect()
}

/// `((0))` → `0`, `(getdate())` → `getdate()`: SQL Server wraps defaults.
fn norm_default(d: &Option<String>) -> Option<String> {
    let mut s: String = d.as_ref()?.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
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
        s.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(';').trim().to_string()
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

fn object_key(kind: ObjectKind, schema: &str, name: &str) -> String {
    format!("{}:{}.{}", kind_word(kind), schema.to_lowercase(), name.to_lowercase())
}

// ---------------------------------------------------------------------------
// DDL text
// ---------------------------------------------------------------------------

fn q(d: Dialect, s: &str) -> String {
    d.quote_ident(s)
}

fn qualified(d: Dialect, schema: &str, name: &str) -> String {
    if schema.is_empty() { q(d, name) } else { format!("{}.{}", q(d, schema), q(d, name)) }
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
        let cols = i.columns.iter().map(|c| quote_index_col(c, d)).collect::<Vec<_>>().join(", ");
        return format!("ALTER TABLE {} ADD CONSTRAINT {} UNIQUE ({cols});", qualified(d, &t.schema, &t.name), q(d, &i.name));
    }
    if let Some(def) = &i.definition {
        return format!("{};", def.trim_end_matches(';'));
    }
    let cols = i.columns.iter().map(|c| quote_index_col(c, d)).collect::<Vec<_>>().join(", ");
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
        s.push_str(&format!(" INCLUDE ({})", i.include.iter().map(|c| q(d, c)).collect::<Vec<_>>().join(", ")));
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
    let body = if def.to_ascii_uppercase().starts_with("CHECK") { def.to_string() } else { format!("CHECK {def}") };
    format!("ALTER TABLE {} ADD CONSTRAINT {} {body};", qualified(d, &t.schema, &t.name), q(d, &c.name))
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
    let mut s = format!("CREATE TABLE {} (\n{}\n);", qualified(d, &t.schema, &t.name), lines.join(",\n"));
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
        let o = CompareOptions { ignore_case: true, ..Default::default() };
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
        && a.filter.as_deref().map(|f| norm_text(f, &CompareOptions { ignore_case: true, ..Default::default() }))
            == b.filter.as_deref().map(|f| norm_text(f, &CompareOptions { ignore_case: true, ..Default::default() }))
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
    let mut t: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
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
        .map(|(_, r)| r.trim_end_matches(')').split(',').map(|x| if x == "max" { Some(i64::MAX) } else { x.parse().ok() }).collect())
        .unwrap_or_default();
    let family = match base.as_str() {
        "nvarchar" | "varchar" | "nchar" | "char" | "charactervarying" | "character" | "text" | "ntext" => "text".to_string(),
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
        return (b < a).then(|| format!("{from} → {to} is smaller: values out of range fail"));
    }
    if ff != tf {
        return Some(format!("{from} → {to} changes the kind of data: values that do not convert fail"));
    }
    if let (Some(a), Some(b)) = (fl, tl)
        && b < a
    {
        return Some(format!("{from} → {to} is shorter: longer values fail or are cut"));
    }
    if let (Some(a), Some(b)) = (fs, ts)
        && b < a
    {
        return Some(format!("{from} → {to} keeps fewer decimals: values are rounded"));
    }
    None
}

fn compare_tables(s: &TableDef, t: &TableDef, opts: &CompareOptions) -> (Vec<String>, Vec<String>) {
    let mut details = Vec::new();
    let mut warnings = Vec::new();
    let col = |cols: &[ColumnDef], n: &str| cols.iter().find(|c| name_key(&c.name, opts) == name_key(n, opts)).cloned();
    for c in &s.columns {
        match col(&t.columns, &c.name) {
            None => {
                details.push(format!("column {} {} added", c.name, c.data_type));
                if !c.nullable && c.default.is_none() && !c.identity && c.computed.is_none() {
                    warnings.push(format!("New column {} is NOT NULL without a default: adding it fails if {} has rows.", c.name, t.name));
                }
            }
            Some(o) => {
                if norm_type(&c.data_type) != norm_type(&o.data_type) {
                    details.push(format!("column {}: type {} → {}", c.name, o.data_type, c.data_type));
                    if let Some(w) = narrowing(&o.data_type, &c.data_type) {
                        warnings.push(format!("Column {}.{}: {w}.", t.name, c.name));
                    }
                }
                if c.nullable != o.nullable {
                    details.push(format!("column {}: {} → {}", c.name, if o.nullable { "NULL" } else { "NOT NULL" }, if c.nullable { "NULL" } else { "NOT NULL" }));
                    if !c.nullable {
                        warnings.push(format!("Column {}.{} becomes NOT NULL: fails if it holds NULLs.", t.name, c.name));
                    }
                }
                if norm_default(&c.default) != norm_default(&o.default) {
                    details.push(format!(
                        "column {}: default {} → {}",
                        c.name,
                        o.default.as_deref().unwrap_or("none"),
                        c.default.as_deref().unwrap_or("none")
                    ));
                }
                if c.identity != o.identity {
                    details.push(format!("column {}: identity {} → {}", c.name, o.identity, c.identity));
                    warnings.push(format!("Column {}.{}: identity cannot be changed in place; the script leaves it for you to do by hand.", t.name, c.name));
                }
                if c.computed.as_deref().map(|x| norm_text(x, opts)) != o.computed.as_deref().map(|x| norm_text(x, opts)) {
                    details.push(format!("column {}: computed expression differs", c.name));
                }
            }
        }
    }
    for o in &t.columns {
        if col(&s.columns, &o.name).is_none() {
            details.push(format!("column {} removed", o.name));
            warnings.push(format!("Column {}.{} is dropped: its data is lost.", t.name, o.name));
        }
    }
    let src_order: Vec<String> = s.columns.iter().map(|c| name_key(&c.name, opts)).collect();
    let tgt_order: Vec<String> = t.columns.iter().map(|c| name_key(&c.name, opts)).filter(|n| src_order.contains(n)).collect();
    let src_common: Vec<String> = src_order.iter().filter(|n| tgt_order.contains(n)).cloned().collect();
    if src_common != tgt_order {
        details.push("column order differs (not changed by the script)".into());
    }
    let pk_cols = |k: &Option<KeyDef>| k.as_ref().map(|k| norm_cols(&k.columns));
    if pk_cols(&s.primary_key) != pk_cols(&t.primary_key) {
        details.push(format!(
            "primary key ({}) → ({})",
            t.primary_key.as_ref().map(|k| k.columns.join(", ")).unwrap_or_default(),
            s.primary_key.as_ref().map(|k| k.columns.join(", ")).unwrap_or_default()
        ));
    }
    for i in &s.indexes {
        match t.indexes.iter().find(|x| name_key(&x.name, opts) == name_key(&i.name, opts)) {
            None => details.push(format!("index {} added", i.name)),
            Some(o) if !same_index(i, o) => details.push(format!("index {} changed", i.name)),
            _ => {}
        }
    }
    for o in &t.indexes {
        if !s.indexes.iter().any(|x| name_key(&x.name, opts) == name_key(&o.name, opts)) {
            details.push(format!("index {} removed", o.name));
        }
    }
    for f in &s.foreign_keys {
        match t.foreign_keys.iter().find(|x| name_key(&x.name, opts) == name_key(&f.name, opts)) {
            None => details.push(format!("foreign key {} added", f.name)),
            Some(o) if !same_fk(f, o) => details.push(format!("foreign key {} changed", f.name)),
            _ => {}
        }
    }
    for o in &t.foreign_keys {
        if !s.foreign_keys.iter().any(|x| name_key(&x.name, opts) == name_key(&o.name, opts)) {
            details.push(format!("foreign key {} removed", o.name));
        }
    }
    for c in &s.checks {
        match t.checks.iter().find(|x| name_key(&x.name, opts) == name_key(&c.name, opts)) {
            None => details.push(format!("check {} added", c.name)),
            Some(o) if norm_check(&c.definition) != norm_check(&o.definition) => details.push(format!("check {} changed", c.name)),
            _ => {}
        }
    }
    for o in &t.checks {
        if !s.checks.iter().any(|x| name_key(&x.name, opts) == name_key(&o.name, opts)) {
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
    let mkey = |m: &ModuleDef| (kind_word(m.kind), name_key(&m.schema, opts), name_key(&m.name, opts));
    let unreadable: HashSet<String> = source.unreadable.iter().chain(&target.unreadable).map(|n| name_key(n, opts)).collect();
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
                out.differences.push(diff(Change::Changed, vec!["definition differs".into()], Some(t.definition.clone())))
            }
            Some(_) => out.identical += 1,
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
    out.differences.sort_by(|a, b| (order(a.change), kind_word(a.kind), a.schema.to_lowercase(), a.name.to_lowercase()).cmp(&(order(b.change), kind_word(b.kind), b.schema.to_lowercase(), b.name.to_lowercase())));
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

#[derive(Default)]
struct Phases {
    create_schemas: Vec<String>,
    drop_fks: Vec<String>,
    /// (rank, statement): dropped in reverse dependency order
    drop_modules: Vec<(u8, String)>,
    drop_indexes: Vec<String>,
    create_tables: Vec<String>,
    alter_tables: Vec<String>,
    drop_tables: Vec<String>,
    add_keys: Vec<String>,
    add_fks: Vec<String>,
    modules: Vec<(u8, String)>,
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
        return format!("DROP TRIGGER IF EXISTS {} ON {};", q(d, &m.name), qualified(d, &m.schema, table));
    }
    if d == Dialect::Postgres && matches!(m.kind, ObjectKind::ScalarFunction | ObjectKind::TableFunction | ObjectKind::Procedure) {
        // overloads: drop by signature is not known here, the name works when unique
        return format!("DROP {what} IF EXISTS {};", qualified(d, &m.schema, &m.name));
    }
    format!("DROP {what} IF EXISTS {};", qualified(d, &m.schema, &m.name))
}

/// SQL Server: CREATE → CREATE OR ALTER for views, routines and triggers.
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
    if rest.len() >= 6 && rest[..6].eq_ignore_ascii_case("CREATE") && !rest[6..].trim_start().to_ascii_uppercase().starts_with("OR ") {
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

/// A script that applies the selected differences (by key) to the target.
pub fn sync_script(
    source: &SchemaModel,
    target: &SchemaModel,
    diff: &SchemaDiff,
    selected: &[String],
    dialect: Dialect,
    opts: &CompareOptions,
) -> SyncScript {
    let d = dialect;
    let pick: HashSet<&str> = selected.iter().map(String::as_str).collect();
    let chosen: Vec<&ObjectDiff> = diff.differences.iter().filter(|x| pick.contains(x.key.as_str())).collect();
    let mut p = Phases::default();
    let mut warnings: Vec<String> = Vec::new();
    let nk = |s: &str| name_key(s, opts);
    let find_table = |m: &SchemaModel, o: &ObjectDiff| m.tables.iter().find(|t| nk(&t.schema) == nk(&o.schema) && nk(&t.name) == nk(&o.name)).cloned();
    let find_module = |m: &SchemaModel, o: &ObjectDiff| {
        m.modules.iter().find(|x| kind_word(x.kind) == kind_word(o.kind) && nk(&x.schema) == nk(&o.schema) && nk(&x.name) == nk(&o.name)).cloned()
    };
    // foreign keys dropped before tables / keys they depend on change
    let mut dropped_fks: HashSet<(String, String)> = HashSet::new();
    let mut drop_fk = |t: &TableDef, f: &ForeignKeyDef, p: &mut Phases| {
        if dropped_fks.insert((nk(&t.name), nk(&f.name))) {
            p.drop_fks.push(drop_constraint(d, t, &f.name, "fk"));
        }
    };
    let references = |tt: &TableDef, schema: &str, name: &str| -> Vec<ForeignKeyDef> {
        tt.foreign_keys.iter().filter(|f| nk(&f.ref_table) == nk(name) && nk(&f.ref_schema) == nk(schema)).cloned().collect()
    };

    for o in &chosen {
        warnings.extend(o.warnings.iter().cloned());
        match (o.kind, o.change) {
            (ObjectKind::Table, Change::Added) => {
                let Some(s) = find_table(source, o) else { continue };
                p.create_tables.push(table_sql(&s, d, false));
                for i in &s.indexes {
                    p.add_keys.push(index_sql(&s, i, d));
                }
                for c in &s.checks {
                    p.add_keys.push(check_sql(&s, c, d));
                }
                for f in &s.foreign_keys {
                    p.add_fks.push(fk_sql(&s, f, d));
                }
            }
            (ObjectKind::Table, Change::Removed) => {
                let Some(t) = find_table(target, o) else { continue };
                // foreign keys of other tables that point at it go first
                for other in &target.tables {
                    for f in references(other, &t.schema, &t.name) {
                        if nk(&other.name) != nk(&t.name) {
                            warnings.push(format!("Foreign key {} on {} references {} and is dropped with it.", f.name, other.name, t.name));
                        }
                        drop_fk(other, &f, &mut p);
                    }
                }
                for f in &t.foreign_keys {
                    drop_fk(&t, f, &mut p);
                }
                p.drop_tables.push(format!("DROP TABLE {};", qualified(d, &t.schema, &t.name)));
            }
            (ObjectKind::Table, Change::Changed) => {
                let (Some(s), Some(t)) = (find_table(source, o), find_table(target, o)) else { continue };
                let table = qualified(d, &t.schema, &t.name);
                let col_in = |cols: &[ColumnDef], n: &str| cols.iter().find(|c| nk(&c.name) == nk(n)).cloned();
                let pk_changed = s.primary_key.as_ref().map(|k| norm_cols(&k.columns)) != t.primary_key.as_ref().map(|k| norm_cols(&k.columns));
                // foreign keys: removed or changed ones are dropped, new or changed ones added
                for f in &t.foreign_keys {
                    let keep = s.foreign_keys.iter().find(|x| nk(&x.name) == nk(&f.name)).is_some_and(|x| same_fk(x, f));
                    if !keep {
                        drop_fk(&t, f, &mut p);
                    }
                }
                for f in &s.foreign_keys {
                    let exists = t.foreign_keys.iter().find(|x| nk(&x.name) == nk(&f.name)).is_some_and(|x| same_fk(x, f));
                    if !exists {
                        p.add_fks.push(fk_sql(&s, f, d));
                    }
                }
                // a changed primary key: foreign keys pointing at it are rebuilt
                if pk_changed {
                    for other in &target.tables {
                        for f in references(other, &t.schema, &t.name) {
                            if nk(&other.name) == nk(&t.name) {
                                continue;
                            }
                            drop_fk(other, &f, &mut p);
                            if let Some(src_other) = source.tables.iter().find(|x| nk(&x.name) == nk(&other.name) && nk(&x.schema) == nk(&other.schema))
                                && let Some(sf) = src_other.foreign_keys.iter().find(|x| nk(&x.name) == nk(&f.name))
                            {
                                p.add_fks.push(fk_sql(src_other, sf, d));
                            } else {
                                warnings.push(format!("Foreign key {} on {} depends on the changed primary key of {} and is dropped.", f.name, other.name, t.name));
                            }
                        }
                    }
                    if let Some(k) = &t.primary_key {
                        p.drop_indexes.push(drop_constraint(d, &t, &k.name, "pk"));
                    }
                    if let Some(pk) = pk_clause(&s, d) {
                        p.add_keys.push(format!("ALTER TABLE {table} ADD {pk};"));
                    }
                }
                // indexes and checks
                for i in &t.indexes {
                    let keep = s.indexes.iter().find(|x| nk(&x.name) == nk(&i.name)).is_some_and(|x| same_index(x, i));
                    if !keep {
                        p.drop_indexes.push(drop_index(d, &t, i));
                    }
                }
                for i in &s.indexes {
                    let exists = t.indexes.iter().find(|x| nk(&x.name) == nk(&i.name)).is_some_and(|x| same_index(x, i));
                    if !exists {
                        p.add_keys.push(index_sql(&s, i, d));
                    }
                }
                for c in &t.checks {
                    let keep = s.checks.iter().find(|x| nk(&x.name) == nk(&c.name)).is_some_and(|x| norm_check(&x.definition) == norm_check(&c.definition));
                    if !keep {
                        p.drop_indexes.push(drop_constraint(d, &t, &c.name, "check"));
                    }
                }
                for c in &s.checks {
                    let exists = t.checks.iter().find(|x| nk(&x.name) == nk(&c.name)).is_some_and(|x| norm_check(&x.definition) == norm_check(&c.definition));
                    if !exists {
                        p.add_keys.push(check_sql(&s, c, d));
                    }
                }
                // columns
                for c in &s.columns {
                    match col_in(&t.columns, &c.name) {
                        None => p.alter_tables.push(match d {
                            Dialect::MsSql => format!("ALTER TABLE {table} ADD {};", column_sql(c, d)),
                            _ => format!("ALTER TABLE {table} ADD COLUMN {};", column_sql(c, d)),
                        }),
                        Some(o) => {
                            let col = q(d, &c.name);
                            let type_changed = norm_type(&c.data_type) != norm_type(&o.data_type);
                            let null_changed = c.nullable != o.nullable;
                            let default_changed = norm_default(&c.default) != norm_default(&o.default);
                            let computed_changed =
                                c.computed.as_deref().map(|x| norm_text(x, opts)) != o.computed.as_deref().map(|x| norm_text(x, opts));
                            if c.identity != o.identity || computed_changed {
                                p.alter_tables.push(format!(
                                    "-- {}.{}: {} differs; recreate the column by hand (Sqlotus does not rebuild columns).",
                                    t.name,
                                    c.name,
                                    if computed_changed { "the computed expression" } else { "identity" }
                                ));
                                if computed_changed {
                                    warnings.push(format!("Column {}.{}: the computed expression differs and must be changed by hand.", t.name, c.name));
                                }
                                continue;
                            }
                            match d {
                                Dialect::MsSql => {
                                    if default_changed && let Some(n) = &o.default_name {
                                        p.alter_tables.push(format!("ALTER TABLE {table} DROP CONSTRAINT {};", q(d, n)));
                                    }
                                    if type_changed || null_changed {
                                        p.alter_tables.push(format!(
                                            "ALTER TABLE {table} ALTER COLUMN {col} {} {};",
                                            c.data_type,
                                            if c.nullable { "NULL" } else { "NOT NULL" }
                                        ));
                                    }
                                    if default_changed && let Some(def) = &c.default {
                                        let name = c.default_name.as_ref().map(|n| format!("CONSTRAINT {} ", q(d, n))).unwrap_or_default();
                                        p.alter_tables.push(format!("ALTER TABLE {table} ADD {name}DEFAULT {def} FOR {col};"));
                                    }
                                }
                                Dialect::Postgres => {
                                    if type_changed {
                                        p.alter_tables.push(format!(
                                            "ALTER TABLE {table} ALTER COLUMN {col} TYPE {} USING {col}::{};",
                                            c.data_type, c.data_type
                                        ));
                                    }
                                    if null_changed {
                                        p.alter_tables.push(format!(
                                            "ALTER TABLE {table} ALTER COLUMN {col} {} NOT NULL;",
                                            if c.nullable { "DROP" } else { "SET" }
                                        ));
                                    }
                                    if default_changed {
                                        p.alter_tables.push(match &c.default {
                                            Some(def) => format!("ALTER TABLE {table} ALTER COLUMN {col} SET DEFAULT {def};"),
                                            None => format!("ALTER TABLE {table} ALTER COLUMN {col} DROP DEFAULT;"),
                                        });
                                    }
                                }
                                Dialect::MySql => {
                                    if type_changed || null_changed || default_changed {
                                        p.alter_tables.push(format!("ALTER TABLE {table} MODIFY COLUMN {};", column_sql(c, d)));
                                    }
                                }
                            }
                        }
                    }
                }
                for o in &t.columns {
                    if col_in(&s.columns, &o.name).is_none() {
                        if d == Dialect::MsSql
                            && let Some(n) = &o.default_name
                        {
                            p.alter_tables.push(format!("ALTER TABLE {table} DROP CONSTRAINT {};", q(d, n)));
                        }
                        p.alter_tables.push(format!("ALTER TABLE {table} DROP COLUMN {};", q(d, &o.name)));
                    }
                }
            }
            (_, Change::Removed) => {
                if let Some(m) = find_module(target, o) {
                    p.drop_modules.push((module_rank(m.kind), drop_module(d, &m)));
                }
            }
            (_, Change::Added) => {
                if let Some(m) = find_module(source, o) {
                    p.modules.push((module_rank(m.kind), m.definition.clone()));
                }
            }
            (_, Change::Changed) => {
                let (Some(m), Some(old)) = (find_module(source, o), find_module(target, o)) else { continue };
                match d {
                    Dialect::MsSql => p.modules.push((module_rank(m.kind), create_or_alter(&m.definition))),
                    Dialect::Postgres if m.kind != ObjectKind::Trigger && m.kind != ObjectKind::View => {
                        // functions and procedures are CREATE OR REPLACE
                        p.modules.push((module_rank(m.kind), m.definition.clone()));
                    }
                    Dialect::Postgres if m.kind == ObjectKind::View => {
                        // a view whose columns change cannot be replaced in place
                        p.drop_modules.push((module_rank(old.kind), drop_module(d, &old)));
                        p.modules.push((module_rank(m.kind), m.definition.clone()));
                        warnings.push(format!("View {} is dropped and created again; views that use it must be created again too.", m.name));
                    }
                    _ => {
                        p.drop_modules.push((module_rank(old.kind), drop_module(d, &old)));
                        p.modules.push((module_rank(m.kind), m.definition.clone()));
                    }
                }
            }
        }
    }
    p.modules.sort_by_key(|(rank, _)| *rank);
    // triggers before the functions they call, views before what they use
    p.drop_modules.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    let drop_modules: Vec<String> = p.drop_modules.iter().map(|(_, s)| s.clone()).collect();
    // schemas that new objects need
    if d != Dialect::MySql {
        let existing: HashSet<String> = target.tables.iter().map(|t| nk(&t.schema)).chain(target.modules.iter().map(|m| nk(&m.schema))).collect();
        let mut needed: Vec<String> = Vec::new();
        for o in &chosen {
            let builtin = matches!(o.schema.to_lowercase().as_str(), "dbo" | "public");
            if o.change == Change::Added && !o.schema.is_empty() && !builtin && !existing.contains(&nk(&o.schema)) && !needed.iter().any(|s| nk(s) == nk(&o.schema)) {
                needed.push(o.schema.clone());
            }
        }
        for s in needed {
            p.create_schemas.push(match d {
                Dialect::MsSql => {
                    let lit = |x: &str| x.replace('\'', "''");
                    format!("IF SCHEMA_ID(N'{}') IS NULL EXEC(N'CREATE SCHEMA {}');", lit(&s), lit(&q(d, &s)))
                }
                _ => format!("CREATE SCHEMA IF NOT EXISTS {};", q(d, &s)),
            });
        }
    }

    let mut script = String::new();
    let mut statements = 0usize;
    let go = if d == Dialect::MsSql { "GO\n" } else { "" };
    script.push_str("-- Schema synchronisation script generated by Sqlotus.\n-- Review it before running: it changes the target database.\n");
    let mut seen = HashSet::new();
    warnings.retain(|w| seen.insert(w.clone()));
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
    if !p.modules.is_empty() {
        // the comment gets its own batch: SQL Server would store it as part of the first module
        script.push_str("\n-- Create or change views, routines and triggers\n");
        script.push_str(go);
        for (_, m) in &p.modules {
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
    SyncScript { script, warnings, statements }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnDef {
        ColumnDef { name: name.into(), data_type: ty.into(), nullable, ..Default::default() }
    }

    fn customers() -> TableDef {
        TableDef {
            schema: "dbo".into(),
            name: "Customers".into(),
            columns: vec![
                ColumnDef { identity: true, ..col("Id", "int", false) },
                col("Name", "nvarchar(100)", false),
                ColumnDef { default: Some("('VN')".into()), default_name: Some("DF_Customers_Country".into()), ..col("Country", "char(2)", true) },
            ],
            primary_key: Some(KeyDef { name: "PK_Customers".into(), columns: vec!["Id".into()] }),
            indexes: vec![IndexDef { name: "IX_Customers_Name".into(), columns: vec!["Name".into()], ..Default::default() }],
            ..Default::default()
        }
    }

    fn orders() -> TableDef {
        TableDef {
            schema: "dbo".into(),
            name: "Orders".into(),
            columns: vec![ColumnDef { identity: true, ..col("Id", "int", false) }, col("CustomerId", "int", false), col("Total", "decimal(12, 2)", false)],
            primary_key: Some(KeyDef { name: "PK_Orders".into(), columns: vec!["Id".into()] }),
            foreign_keys: vec![ForeignKeyDef {
                name: "FK_Orders_Customers".into(),
                columns: vec!["CustomerId".into()],
                ref_schema: "dbo".into(),
                ref_table: "Customers".into(),
                ref_columns: vec!["Id".into()],
                ..Default::default()
            }],
            checks: vec![CheckDef { name: "CK_Orders_Total".into(), definition: "([Total]>=(0))".into() }],
            ..Default::default()
        }
    }

    fn view(def: &str) -> ModuleDef {
        ModuleDef { schema: "dbo".into(), name: "vTotals".into(), kind: ObjectKind::View, definition: def.into(), table: None }
    }

    fn model() -> SchemaModel {
        SchemaModel { unreadable: vec![], tables: vec![customers(), orders()], modules: vec![view("CREATE VIEW dbo.vTotals AS\nSELECT CustomerId, SUM(Total) AS Total FROM dbo.Orders GROUP BY CustomerId")] }
    }

    const MS: Dialect = Dialect::MsSql;

    #[test]
    fn identical_and_empty() {
        let opts = CompareOptions::default();
        let d = compare(&model(), &model(), MS, &opts);
        assert!(d.differences.is_empty());
        assert_eq!(d.identical, 3);
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
        let ws = CompareOptions { ignore_case: true, ..Default::default() };
        assert!(compare(&model(), &t, MS, &ws).differences.is_empty());
        // without ignoring case the view text differs
        let strict = CompareOptions { ignore_case: false, ..Default::default() };
        let d = compare(&model(), &t, MS, &strict);
        assert_eq!(d.differences.len(), 1);
        assert_eq!(d.differences[0].kind, ObjectKind::View);
        // case-sensitive names (Postgres): CUSTOMERS is another table
        let pg = CompareOptions { case_insensitive_names: false, ignore_case: true, ..Default::default() };
        let d = compare(&model(), &t, Dialect::Postgres, &pg);
        assert_eq!(d.differences.iter().filter(|x| x.kind == ObjectKind::Table).count(), 2);
    }

    #[test]
    fn table_changes_and_warnings() {
        let src = model();
        let mut tgt = model();
        let c = &mut tgt.tables[0];
        c.columns[1].data_type = "nvarchar(200)".into(); // source is shorter → narrowing
        c.columns[2].nullable = false; // source allows NULL
        c.columns.push(col("Legacy", "int", true)); // dropped
        c.indexes.clear(); // index added
        tgt.tables[1].columns.retain(|x| x.name != "Total");
        tgt.tables[1].checks.clear();
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let cust = d.differences.iter().find(|x| x.name == "Customers").unwrap();
        assert!(cust.details.contains(&"column Name: type nvarchar(200) → nvarchar(100)".to_string()), "{:?}", cust.details);
        assert!(cust.details.contains(&"column Legacy removed".to_string()));
        assert!(cust.details.contains(&"index IX_Customers_Name added".to_string()));
        assert!(cust.warnings.iter().any(|w| w.contains("shorter")));
        assert!(cust.warnings.iter().any(|w| w.contains("Legacy is dropped")));
        let ord = d.differences.iter().find(|x| x.name == "Orders").unwrap();
        assert!(ord.warnings.iter().any(|w| w.contains("NOT NULL without a default")));
        let keys: Vec<String> = d.differences.iter().map(|x| x.key.clone()).collect();
        let s = sync_script(&src, &tgt, &d, &keys, MS, &opts);
        assert!(s.script.contains("ALTER TABLE dbo.Customers ALTER COLUMN Name nvarchar(100) NOT NULL;"), "{}", s.script);
        assert!(s.script.contains("ALTER TABLE dbo.Customers ALTER COLUMN Country char(2) NULL;"));
        assert!(s.script.contains("ALTER TABLE dbo.Customers DROP COLUMN Legacy;"));
        assert!(s.script.contains("CREATE NONCLUSTERED INDEX IX_Customers_Name ON dbo.Customers (Name);"));
        assert!(s.script.contains("ALTER TABLE dbo.Orders ADD Total decimal(12, 2) NOT NULL;"));
        assert!(s.script.contains("ALTER TABLE dbo.Orders ADD CONSTRAINT CK_Orders_Total CHECK ([Total]>=(0));"));
        assert!(s.script.contains("-- WARNINGS:"));
        // the add-column phase comes before the check that uses it
        assert!(s.script.find("ADD Total").unwrap() < s.script.find("CK_Orders_Total CHECK").unwrap());
    }

    #[test]
    fn adding_and_dropping_tables_in_dependency_order() {
        let src = model();
        let tgt = SchemaModel { tables: vec![], modules: vec![], ..Default::default() };
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
        assert!(s.script.contains("CREATE VIEW dbo.vTotals AS\nSELECT CustomerId, SUM(Total) AS Total FROM dbo.Orders GROUP BY CustomerId\nGO"));

        // the reverse: dropping Customers alone also drops the foreign key pointing at it
        let d = compare(&tgt, &src, MS, &opts);
        let only_customers = vec!["table:dbo.customers".to_string()];
        let s = sync_script(&tgt, &src, &d, &only_customers, MS, &opts);
        assert!(s.script.find("DROP CONSTRAINT FK_Orders_Customers").unwrap() < s.script.find("DROP TABLE dbo.Customers").unwrap());
        assert!(s.warnings.iter().any(|w| w.contains("FK_Orders_Customers on Orders references Customers")));
        assert!(s.warnings.iter().any(|w| w.contains("dropped with all its data")));
    }

    #[test]
    fn primary_key_change_rebuilds_dependent_foreign_keys() {
        let src = model();
        let mut tgt = model();
        tgt.tables[0].primary_key = Some(KeyDef { name: "PK_Old".into(), columns: vec!["Name".into()] });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], MS, &opts);
        let drop_fk = s.script.find("ALTER TABLE dbo.Orders DROP CONSTRAINT FK_Orders_Customers;").unwrap();
        let drop_pk = s.script.find("DROP CONSTRAINT PK_Old").unwrap();
        let add_pk = s.script.find("ADD CONSTRAINT PK_Customers PRIMARY KEY (Id)").unwrap();
        let add_fk = s.script.find("ALTER TABLE dbo.Orders ADD CONSTRAINT FK_Orders_Customers").unwrap();
        assert!(drop_fk < drop_pk && drop_pk < add_pk && add_pk < add_fk, "{}", s.script);
    }

    #[test]
    fn modules_per_dialect() {
        let src = model();
        let mut tgt = model();
        tgt.modules[0].definition = "CREATE VIEW dbo.vTotals AS SELECT 1 AS x".into();
        tgt.modules.push(ModuleDef { schema: "dbo".into(), name: "pOld".into(), kind: ObjectKind::Procedure, definition: "CREATE PROCEDURE dbo.pOld AS SELECT 1".into(), table: None });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let keys: Vec<String> = d.differences.iter().map(|x| x.key.clone()).collect();
        let s = sync_script(&src, &tgt, &d, &keys, MS, &opts);
        assert!(s.script.contains("CREATE OR ALTER VIEW dbo.vTotals AS"), "{}", s.script);
        assert!(s.script.contains("DROP PROCEDURE IF EXISTS dbo.pOld;"));
        let s = sync_script(&src, &tgt, &d, &keys, Dialect::MySql, &opts);
        assert!(s.script.find("DROP VIEW IF EXISTS").unwrap() < s.script.find("CREATE VIEW dbo.vTotals").unwrap());
        assert_eq!(create_or_alter("-- note\nCREATE   PROCEDURE p AS SELECT 1"), "-- note\nCREATE OR ALTER   PROCEDURE p AS SELECT 1");
        assert_eq!(create_or_alter("CREATE OR ALTER VIEW v AS SELECT 1"), "CREATE OR ALTER VIEW v AS SELECT 1");
        let o = CompareOptions::default();
        assert_eq!(norm_module("CREATE OR ALTER VIEW v AS SELECT 1", &o), norm_module("create   view v AS SELECT 1", &CompareOptions { ignore_case: false, ..o }).replace("create", "CREATE").replace("view", "VIEW"));
        assert_eq!(norm_module("ALTER PROCEDURE p AS SELECT 1", &o), norm_module("CREATE PROCEDURE p AS SELECT 1", &o));
        assert_eq!(norm_module("CREATE OR REPLACE VIEW v AS SELECT 1", &o), "CREATE VIEW v AS SELECT 1");
    }

    #[test]
    fn reserved_and_unusual_names_are_quoted() {
        let t = TableDef {
            schema: "dbo".into(),
            name: "Order Lines".into(),
            columns: vec![col("LineNo", "int", false), col("User", "nvarchar(10)", true)],
            primary_key: Some(KeyDef { name: "PK_Lines".into(), columns: vec!["LineNo".into()] }),
            ..Default::default()
        };
        let src = SchemaModel { tables: vec![t], modules: vec![], ..Default::default() };
        let d = compare(&src, &SchemaModel::default(), MS, &CompareOptions::default());
        let s = sync_script(&src, &SchemaModel::default(), &d, &[d.differences[0].key.clone()], MS, &CompareOptions::default());
        assert!(!s.script.contains("CREATE SCHEMA"), "dbo exists when the target has objects in it");
        let sales = TableDef { schema: "sales".into(), name: "Regions".into(), columns: vec![col("Code", "char(2)", false)], ..Default::default() };
        let with_sales = SchemaModel { tables: vec![sales], modules: vec![], ..Default::default() };
        let target = SchemaModel { tables: vec![customers()], modules: vec![], ..Default::default() };
        let d2 = compare(&with_sales, &target, MS, &CompareOptions::default());
        let added: Vec<String> = d2.differences.iter().filter(|x| x.change == Change::Added).map(|x| x.key.clone()).collect();
        let s2 = sync_script(&with_sales, &target, &d2, &added, MS, &CompareOptions::default());
        assert!(s2.script.find("IF SCHEMA_ID(N'sales') IS NULL EXEC(N'CREATE SCHEMA sales');").unwrap() < s2.script.find("CREATE TABLE sales.Regions").unwrap());
        let s3 = sync_script(&with_sales, &target, &d2, &added, Dialect::Postgres, &CompareOptions::default());
        assert!(s3.script.contains("CREATE SCHEMA IF NOT EXISTS sales;"));
        assert!(s.script.contains("CREATE TABLE dbo.[Order Lines] (\n    [LineNo] int NOT NULL,\n    [User] nvarchar(10) NULL,\n    CONSTRAINT PK_Lines PRIMARY KEY ([LineNo])"), "{}", s.script);
    }

    #[test]
    fn unique_constraints_and_expression_indexes() {
        let uq = IndexDef { name: "UQ_Name".into(), columns: vec!["Name".into()], unique: true, constraint: true, ..Default::default() };
        let mut src = model();
        src.tables[0].indexes.push(uq.clone());
        let mut tgt = model();
        tgt.tables[0].indexes.push(IndexDef { columns: vec!["Name".into(), "Country".into()], ..uq });
        let opts = CompareOptions::default();
        let d = compare(&src, &tgt, MS, &opts);
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], MS, &opts);
        assert!(s.script.contains("ALTER TABLE dbo.Customers DROP CONSTRAINT UQ_Name;"), "{}", s.script);
        assert!(s.script.contains("ALTER TABLE dbo.Customers ADD CONSTRAINT UQ_Name UNIQUE (Name);"));
        let a = IndexDef { name: "ix".into(), definition: Some("CREATE INDEX ix ON public.t USING btree (lower(name))".into()), ..Default::default() };
        let b = IndexDef { definition: Some("CREATE INDEX ix ON public.t USING btree (upper(name))".into()), ..a.clone() };
        assert!(same_index(&a, &a.clone()) && !same_index(&a, &b));
    }

    #[test]
    fn unreadable_definitions_are_skipped_not_dropped() {
        // the source login cannot read the view: it must not look removed
        let src = SchemaModel { modules: vec![], unreadable: vec!["dbo.vTotals".into()], ..model() };
        let d = compare(&src, &model(), MS, &CompareOptions::default());
        assert!(d.differences.iter().all(|x| x.kind != ObjectKind::View), "{:?}", d.differences);
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
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], Dialect::Postgres, &opts);
        assert!(s.script.contains("ALTER TABLE dbo.\"Customers\" ALTER COLUMN \"Name\" TYPE nvarchar(100) USING \"Name\"::nvarchar(100);"), "{}", s.script);
        assert!(s.script.contains("ALTER COLUMN \"Country\" SET DEFAULT ('VN');"));
        let s = sync_script(&src, &tgt, &d, &["table:dbo.customers".into()], Dialect::MySql, &opts);
        assert!(s.script.contains("ALTER TABLE dbo.Customers MODIFY COLUMN Name nvarchar(100) NOT NULL;"), "{}", s.script);
        assert!(!s.script.contains("GO"));
    }
}
