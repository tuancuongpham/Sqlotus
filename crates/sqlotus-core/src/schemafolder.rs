//! A database schema kept as files in a folder, usually a git repository:
//! one `.sql` file per object for people and pull requests, and
//! `sqlotus-schema.json`, the snapshot Sqlotus compares with.
//!
//! Changes are classified against a *base*: the last state the database and
//! the folder agreed on. A database object that moved away from the base is
//! a local change (commit it); a folder object that moved away is incoming
//! (get it into the database); both moving is a conflict. Pure: the desktop
//! app reads and writes the files and runs git.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Catalog, Column, DbObject, ForeignKey, ObjectKind};
use crate::schemadiff::{Change, CompareOptions, SchemaModel, compare, object_key, table_sql};

/// The snapshot file at the root of the folder.
pub const SNAPSHOT_FILE: &str = "sqlotus-schema.json";
/// Optional scripts put before / after the statements of "Get latest"
/// (data moves, renames).
pub const PRE_SCRIPT: &str = "Scripts/pre-deploy.sql";
pub const POST_SCRIPT: &str = "Scripts/post-deploy.sql";
/// Folders holding one file per object (written and cleaned by Sqlotus).
pub const OBJECT_FOLDERS: &[&str] = &[
    "Tables",
    "Views",
    "Procedures",
    "Functions",
    "Triggers",
    "Synonyms",
    "Sequences",
];
const FORMAT: u32 = 1;

/// Contents of `sqlotus-schema.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub format: u32,
    pub dialect: Dialect,
    pub model: SchemaModel,
}

impl Snapshot {
    pub fn new(dialect: Dialect, model: SchemaModel) -> Self {
        Self {
            format: FORMAT,
            dialect,
            model: normalized(model),
        }
    }

    /// Stable, pretty JSON (sorted objects), so commits diff cleanly.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let s: Snapshot = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{SNAPSHOT_FILE} is not a Sqlotus schema snapshot: {e}"))?;
        if s.format > FORMAT {
            return Err(format!(
                "{SNAPSHOT_FILE} was written by a newer Sqlotus (format {}); update Sqlotus.",
                s.format
            ));
        }
        Ok(s)
    }
}

/// Objects sorted by kind and name; nested lists sorted by name. Column
/// order is kept (it is part of the table).
pub fn normalized(mut m: SchemaModel) -> SchemaModel {
    let lower = |s: &str| s.to_lowercase();
    m.tables.sort_by_key(|t| (lower(&t.schema), lower(&t.name)));
    for t in &mut m.tables {
        t.indexes.sort_by_key(|i| lower(&i.name));
        t.foreign_keys.sort_by_key(|f| lower(&f.name));
        t.checks.sort_by_key(|c| lower(&c.name));
    }
    m.modules
        .sort_by_key(|x| (object_key(x.kind, &x.schema, &x.name), x.name.clone()));
    m.unreadable.sort_by_key(|u| lower(u));
    m.unreadable.dedup();
    m
}

fn folder_of(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Table => "Tables",
        ObjectKind::View => "Views",
        ObjectKind::Procedure => "Procedures",
        ObjectKind::ScalarFunction | ObjectKind::TableFunction => "Functions",
        ObjectKind::Trigger => "Triggers",
        ObjectKind::Synonym => "Synonyms",
        ObjectKind::Sequence => "Sequences",
    }
}

/// `schema.name.sql`, safe as a Windows file name.
fn file_name(schema: &str, name: &str) -> String {
    let base = if schema.is_empty() {
        name.to_string()
    } else {
        format!("{schema}.{name}")
    };
    let mut clean: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while clean.ends_with(['.', ' ']) {
        clean.pop();
    }
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2", "LPT3",
    ];
    if clean.is_empty() || RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&clean)) {
        clean.insert(0, '_');
    }
    format!("{clean}.sql")
}

/// The per-object files of `model`: (path relative to the folder with `/`,
/// contents). Two objects whose names differ only in case share a file name
/// on Windows, so the second gets a numbered name.
pub fn object_files(model: &SchemaModel, dialect: Dialect) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut add = |kind: ObjectKind, schema: &str, name: &str, body: String| {
        let folder = folder_of(kind);
        let mut path = format!("{folder}/{}", file_name(schema, name));
        let mut n = 2;
        while !taken.insert(path.to_lowercase()) {
            path = format!("{folder}/{}", file_name(schema, &format!("{name}~{n}")));
            n += 1;
        }
        let mut text = body.trim_end().to_string();
        text.push('\n');
        out.push((path, text));
    };
    let model = normalized(model.clone());
    for t in &model.tables {
        add(ObjectKind::Table, &t.schema, &t.name, table_sql(t, dialect, true));
    }
    for m in &model.modules {
        add(m.kind, &m.schema, &m.name, m.definition.clone());
    }
    out
}

/// Where an object differs between the database and the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    /// Changed in the database: commit it to the folder.
    Local,
    /// Changed in the folder: get it into the database.
    Incoming,
    /// Changed on both sides (or no common base yet): choose a side.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItem {
    pub key: String,
    pub kind: ObjectKind,
    pub schema: String,
    pub name: String,
    pub state: SyncState,
    /// The database compared with the folder: `added` = only in the
    /// database, `removed` = only in the folder.
    pub change: Change,
    pub details: Vec<String>,
    pub db_text: Option<String>,
    pub folder_text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub items: Vec<SyncItem>,
    /// Objects that are the same on both sides.
    pub in_sync: usize,
    /// Objects one side could not read.
    pub skipped: Vec<String>,
    /// The new base: objects in sync take the folder version, the others
    /// keep the old base version (or stay out of it).
    #[serde(skip)]
    pub base: SchemaModel,
}

/// Keys of the objects that differ between `a` and `b`.
fn changed_keys(a: &SchemaModel, b: &SchemaModel, dialect: Dialect, opts: &CompareOptions) -> HashSet<String> {
    compare(a, b, dialect, opts)
        .differences
        .into_iter()
        .map(|d| d.key)
        .collect()
}

/// Classify every difference between the database and the folder.
pub fn sync_status(
    db: &SchemaModel,
    folder: &SchemaModel,
    base: Option<&SchemaModel>,
    dialect: Dialect,
    opts: &CompareOptions,
) -> SyncStatus {
    let diff = compare(db, folder, dialect, opts);
    let folder_empty = folder.tables.is_empty() && folder.modules.is_empty();
    let (local, incoming) = match base {
        Some(b) => (
            changed_keys(db, b, dialect, opts),
            changed_keys(folder, b, dialect, opts),
        ),
        None => Default::default(),
    };
    let items = diff
        .differences
        .iter()
        .map(|d| {
            let state = match (base, local.contains(&d.key), incoming.contains(&d.key)) {
                // first commit into an empty folder
                (None, ..) if folder_empty => SyncState::Local,
                (None, ..) => SyncState::Conflict,
                (Some(_), true, false) => SyncState::Local,
                (Some(_), false, true) => SyncState::Incoming,
                (Some(_), ..) => SyncState::Conflict,
            };
            SyncItem {
                key: d.key.clone(),
                kind: d.kind,
                schema: d.schema.clone(),
                name: d.name.clone(),
                state,
                change: d.change,
                details: d.details.clone(),
                db_text: d.source_text.clone(),
                folder_text: d.target_text.clone(),
            }
        })
        .collect();
    let same: Vec<String> = diff.identical_objects.iter().map(|o| o.key.clone()).collect();
    let differing: Vec<String> = diff.differences.iter().map(|d| d.key.clone()).collect();
    // objects in sync move the base to the folder version; differing ones keep the old base
    let mut new_base = with_objects(&SchemaModel::default(), folder, &same);
    if let Some(b) = base {
        new_base = with_objects(&new_base, b, &differing);
    }
    SyncStatus {
        items,
        in_sync: diff.identical,
        skipped: diff.skipped,
        base: normalized(new_base),
    }
}

fn table_key(t: &crate::schemadiff::TableDef) -> String {
    object_key(ObjectKind::Table, &t.schema, &t.name)
}

fn module_key(m: &crate::schemadiff::ModuleDef) -> String {
    object_key(m.kind, &m.schema, &m.name)
}

/// `into` with the objects named by `keys` taken from `from`: replaced when
/// `from` has them, removed when it does not.
pub fn with_objects(into: &SchemaModel, from: &SchemaModel, keys: &[String]) -> SchemaModel {
    let keys: HashSet<&str> = keys.iter().map(String::as_str).collect();
    let mut out = into.clone();
    out.tables.retain(|t| !keys.contains(table_key(t).as_str()));
    out.modules.retain(|m| !keys.contains(module_key(m).as_str()));
    out.tables.extend(
        from.tables
            .iter()
            .filter(|t| keys.contains(table_key(t).as_str()))
            .cloned(),
    );
    out.modules.extend(
        from.modules
            .iter()
            .filter(|m| keys.contains(module_key(m).as_str()))
            .cloned(),
    );
    normalized(out)
}

/// What the folder holds after committing `keys` from the database, and
/// the base after the commit.
pub fn commit(
    db: &SchemaModel,
    folder: &SchemaModel,
    status_base: &SchemaModel,
    keys: &[String],
) -> (SchemaModel, SchemaModel) {
    let new_folder = with_objects(folder, db, keys);
    let new_base = with_objects(status_base, &new_folder, keys);
    (new_folder, new_base)
}

/// The objects of a schema folder as a catalog, so the analysis can check
/// table and column names without a database (`sqlotus-lint --schema`).
/// MySQL folders have no schema names: `database` stands in for them.
pub fn catalog(model: &SchemaModel, dialect: Dialect, database: &str) -> Catalog {
    let schema_of = |s: &str| {
        if s.is_empty() {
            database.to_string()
        } else {
            s.to_string()
        }
    };
    let default_schema = match dialect {
        Dialect::MsSql => "dbo".to_string(),
        Dialect::Postgres => "public".to_string(),
        Dialect::MySql => database.to_string(),
    };
    let mut schemas: BTreeSet<String> = BTreeSet::new();
    let mut objects = Vec::new();
    let mut foreign_keys = Vec::new();
    for t in &model.tables {
        let schema = schema_of(&t.schema);
        schemas.insert(schema.clone());
        let pk: Vec<String> = t
            .primary_key
            .as_ref()
            .map(|k| k.columns.iter().map(|c| c.to_lowercase()).collect())
            .unwrap_or_default();
        objects.push(DbObject {
            schema: schema.clone(),
            name: t.name.clone(),
            kind: ObjectKind::Table,
            columns: t
                .columns
                .iter()
                .map(|c| Column {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                    nullable: c.nullable,
                    is_primary_key: pk.contains(&c.name.to_lowercase()),
                    is_identity: c.identity,
                    is_computed: c.computed.is_some(),
                })
                .collect(),
            params: vec![],
        });
        for f in &t.foreign_keys {
            foreign_keys.push(ForeignKey {
                name: f.name.clone(),
                from_schema: schema.clone(),
                from_table: t.name.clone(),
                from_columns: f.columns.clone(),
                to_schema: schema_of(&f.ref_schema),
                to_table: f.ref_table.clone(),
                to_columns: f.ref_columns.clone(),
            });
        }
    }
    for m in &model.modules {
        let schema = schema_of(&m.schema);
        schemas.insert(schema.clone());
        // views and routines without their columns: names are known, columns are not checked
        objects.push(DbObject {
            schema,
            name: m.name.clone(),
            kind: m.kind,
            columns: vec![],
            params: vec![],
        });
    }
    Catalog {
        database: database.to_string(),
        default_schema,
        schemas: schemas.into_iter().collect(),
        objects,
        foreign_keys,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemadiff::{ColumnDef, KeyDef, ModuleDef, TableDef};

    const MS: Dialect = Dialect::MsSql;

    fn table(name: &str, cols: &[(&str, &str)]) -> TableDef {
        TableDef {
            schema: "dbo".into(),
            name: name.into(),
            columns: cols
                .iter()
                .map(|(n, t)| ColumnDef {
                    name: (*n).into(),
                    data_type: (*t).into(),
                    ..Default::default()
                })
                .collect(),
            primary_key: Some(KeyDef {
                name: format!("PK_{name}"),
                columns: vec![cols[0].0.into()],
            }),
            ..Default::default()
        }
    }

    fn view(name: &str, body: &str) -> ModuleDef {
        ModuleDef {
            schema: "dbo".into(),
            name: name.into(),
            kind: ObjectKind::View,
            definition: format!("CREATE VIEW dbo.{name} AS {body}"),
            table: None,
        }
    }

    fn model(tables: Vec<TableDef>, modules: Vec<ModuleDef>) -> SchemaModel {
        SchemaModel {
            tables,
            modules,
            unreadable: vec![],
        }
    }

    fn states(s: &SyncStatus) -> Vec<(String, SyncState)> {
        s.items.iter().map(|i| (i.name.clone(), i.state)).collect()
    }

    #[test]
    fn snapshot_round_trip_is_stable() {
        let m = model(
            vec![table("Orders", &[("Id", "int")]), table("Customers", &[("Id", "int")])],
            vec![view("vB", "SELECT 1"), view("vA", "SELECT 2")],
        );
        let snap = Snapshot::new(MS, m);
        let json = snap.to_json();
        assert_eq!(snap.model.tables[0].name, "Customers", "sorted by name");
        let back = Snapshot::from_json(&json).unwrap();
        assert_eq!(back, snap);
        assert_eq!(back.to_json(), json, "writing again gives the same bytes");
        let newer = json.replacen("\"format\": 1", "\"format\": 99", 1);
        assert!(Snapshot::from_json(&newer).unwrap_err().contains("newer Sqlotus"));
        assert!(Snapshot::from_json("{ nope").is_err());
    }

    #[test]
    fn one_file_per_object() {
        let mut odd = table("A:B?", &[("Id", "int")]);
        odd.schema = "sales".into();
        let m = model(
            vec![
                table("Orders", &[("Id", "int")]),
                odd,
                table("orders", &[("Id", "int")]),
            ],
            vec![view("vTotals", "SELECT 1")],
        );
        let files = object_files(&m, MS);
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "Tables/dbo.Orders.sql",
                "Tables/dbo.orders~2.sql",
                "Tables/sales.A_B_.sql",
                "Views/dbo.vTotals.sql"
            ]
        );
        assert!(files[0].1.starts_with("CREATE TABLE dbo.Orders (") && files[0].1.ends_with(");\n"));
        assert_eq!(files[3].1, "CREATE VIEW dbo.vTotals AS SELECT 1\n");
        assert_eq!(file_name("", "CON"), "_CON.sql");
        assert_eq!(file_name("dbo", "x. "), "dbo.x.sql");
    }

    #[test]
    fn first_commit_and_unknown_base() {
        let db = model(vec![table("Orders", &[("Id", "int")])], vec![view("v", "SELECT 1")]);
        // empty folder: everything is a local change
        let s = sync_status(&db, &SchemaModel::default(), None, MS, &CompareOptions::default());
        assert!(
            s.items
                .iter()
                .all(|i| i.state == SyncState::Local && i.change == Change::Added)
        );
        // a folder that already has different content and no base: we cannot tell who changed it
        let folder = model(vec![table("Orders", &[("Id", "bigint")])], vec![view("v", "SELECT 1")]);
        let s = sync_status(&db, &folder, None, MS, &CompareOptions::default());
        assert_eq!(states(&s), vec![("Orders".into(), SyncState::Conflict)]);
        assert_eq!(s.in_sync, 1);
        // the base keeps only what both sides agree on
        assert_eq!(s.base.modules.len(), 1);
        assert!(s.base.tables.is_empty());
    }

    #[test]
    fn local_incoming_and_conflict() {
        let o = CompareOptions::default();
        let base = model(
            vec![
                table("A", &[("Id", "int")]),
                table("B", &[("Id", "int")]),
                table("C", &[("Id", "int")]),
            ],
            vec![view("v", "SELECT 1")],
        );
        // database: A changed, view dropped, D created; folder: B changed; both: C changed differently
        let db = model(
            vec![
                table("A", &[("Id", "int"), ("Note", "nvarchar(10)")]),
                table("B", &[("Id", "int")]),
                table("C", &[("Id", "int"), ("X", "int")]),
                table("D", &[("Id", "int")]),
            ],
            vec![],
        );
        let folder = model(
            vec![
                table("A", &[("Id", "int")]),
                table("B", &[("Id", "bigint")]),
                table("C", &[("Id", "int"), ("Y", "int")]),
            ],
            vec![view("v", "SELECT 1")],
        );
        let s = sync_status(&db, &folder, Some(&base), MS, &o);
        let mut got = states(&s);
        got.sort();
        assert_eq!(
            got,
            vec![
                ("A".into(), SyncState::Local),
                ("B".into(), SyncState::Incoming),
                ("C".into(), SyncState::Conflict),
                ("D".into(), SyncState::Local),
                ("v".into(), SyncState::Local),
            ]
        );
        let dropped = s.items.iter().find(|i| i.name == "v").unwrap();
        assert_eq!(dropped.change, Change::Removed, "only in the folder");
        // commit A, D and the dropped view; leave the conflict
        let keys: Vec<String> = s
            .items
            .iter()
            .filter(|i| i.state == SyncState::Local)
            .map(|i| i.key.clone())
            .collect();
        let (new_folder, new_base) = commit(&db, &folder, &s.base, &keys);
        assert!(new_folder.modules.is_empty());
        assert_eq!(new_folder.tables.len(), 4);
        let after = sync_status(&db, &new_folder, Some(&new_base), MS, &o);
        let mut got = states(&after);
        got.sort();
        // what was committed is in sync; B stays incoming (not reverted), C stays a conflict
        assert_eq!(
            got,
            vec![("B".into(), SyncState::Incoming), ("C".into(), SyncState::Conflict)]
        );
        // after the database got B (the "Get latest" script ran), B is in sync and the base follows
        let mut db2 = db.clone();
        db2.tables[1] = table("B", &[("Id", "bigint")]);
        let later = sync_status(&db2, &new_folder, Some(&after.base), MS, &o);
        assert_eq!(states(&later), vec![("C".into(), SyncState::Conflict)]);
        // a teammate then changes B again: it is incoming, not a false conflict
        let mut folder3 = new_folder.clone();
        folder3.tables[1] = table("B", &[("Id", "bigint"), ("Z", "int")]);
        let again = sync_status(&db2, &folder3, Some(&later.base), MS, &o);
        assert!(
            again
                .items
                .iter()
                .any(|i| i.name == "B" && i.state == SyncState::Incoming)
        );
    }

    #[test]
    fn folder_as_catalog() {
        let mut orders = table("Orders", &[("Id", "int"), ("Total", "decimal(12, 2)")]);
        orders.foreign_keys.push(crate::schemadiff::ForeignKeyDef {
            name: "FK".into(),
            columns: vec!["Id".into()],
            ref_schema: "dbo".into(),
            ref_table: "Customers".into(),
            ref_columns: vec!["Id".into()],
            ..Default::default()
        });
        let m = model(vec![orders], vec![view("vTotals", "SELECT 1")]);
        let c = catalog(&m, MS, "Shop");
        assert_eq!(c.default_schema, "dbo");
        assert_eq!(c.objects.len(), 2);
        assert!(c.objects[0].columns[0].is_primary_key);
        assert_eq!(c.foreign_keys.len(), 1);
        // the analysis now knows table and column names
        let found = crate::analysis::diagnostics("SELECT o.Totl FROM dbo.Ordrs x, dbo.Orders o", MS, Some(&c));
        let codes: Vec<&str> = found.iter().map(|d| d.code.as_str()).collect();
        assert!(codes.contains(&"SQ006") && codes.contains(&"SQ007"), "{codes:?}");
        // a view's columns are unknown: no false alarms
        let ok = crate::analysis::diagnostics("SELECT v.Anything FROM dbo.vTotals v", MS, Some(&c));
        assert!(ok.iter().all(|d| d.code != "SQ006" && d.code != "SQ007"), "{ok:?}");
        // MySQL folders have no schema names
        let mut my = m.clone();
        my.tables[0].schema = String::new();
        let c = catalog(&my, Dialect::MySql, "shop");
        assert_eq!(c.objects[0].schema, "shop");
        assert_eq!(c.default_schema, "shop");
    }
}
