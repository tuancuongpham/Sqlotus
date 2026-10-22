//! Shared by the integration tests: the corpus of tricky SQL for each
//! dialect (`tests/corpus`), a small catalog with Unicode names, and a
//! seeded random number generator so failures can be replayed.
#![allow(dead_code)]

use sqlotus_core::Dialect;
use sqlotus_core::catalog::{Catalog, Column, DbObject, ForeignKey, ObjectKind, Param};

pub const DIALECTS: [Dialect; 3] = [Dialect::MsSql, Dialect::Postgres, Dialect::MySql];

/// `(name, text)` of every corpus case, once with LF and once with CRLF
/// line ends.
pub fn corpus(dialect: Dialect) -> Vec<(String, String)> {
    let text = match dialect {
        Dialect::MsSql => include_str!("../corpus/mssql.sql"),
        Dialect::Postgres => include_str!("../corpus/postgres.sql"),
        Dialect::MySql => include_str!("../corpus/mysql.sql"),
    };
    let mut out = Vec::new();
    for part in text.replace("\r\n", "\n").split("-- @case ").skip(1) {
        let (name, body) = part.split_once('\n').expect("a case name line");
        let body = format!("{}\n", body.trim_end());
        out.push((format!("{dialect:?} {}", name.trim()), body.clone()));
        out.push((
            format!("{dialect:?} {} (CRLF)", name.trim()),
            body.replace('\n', "\r\n"),
        ));
    }
    assert!(out.len() >= 20, "corpus for {dialect:?} not found");
    out
}

pub fn col(name: &str, ty: &str, nullable: bool) -> Column {
    Column {
        name: name.into(),
        data_type: ty.into(),
        nullable,
        is_primary_key: name == "Id",
        is_identity: false,
        is_computed: false,
    }
}

/// Customers and Orders (with a foreign key and Vietnamese column names)
/// and a procedure.
pub fn catalog() -> Catalog {
    Catalog {
        database: "Shop".into(),
        default_schema: "dbo".into(),
        schemas: vec!["dbo".into()],
        objects: vec![
            DbObject {
                schema: "dbo".into(),
                name: "Customers".into(),
                kind: ObjectKind::Table,
                columns: vec![
                    col("Id", "int", false),
                    col("Code", "varchar(10)", true),
                    col("Tên", "nvarchar(50)", true),
                ],
                params: vec![],
            },
            DbObject {
                schema: "dbo".into(),
                name: "Orders".into(),
                kind: ObjectKind::Table,
                columns: vec![
                    col("Id", "int", false),
                    col("CustomerId", "int", false),
                    col("NgàyĐặt", "datetime2", false),
                ],
                params: vec![],
            },
            DbObject {
                schema: "dbo".into(),
                name: "GetOrders".into(),
                kind: ObjectKind::Procedure,
                columns: vec![],
                params: vec![Param {
                    name: "@CustomerId".into(),
                    data_type: "int".into(),
                    is_output: false,
                    has_default: false,
                }],
            },
        ],
        foreign_keys: vec![ForeignKey {
            name: "FK_Orders_Customers".into(),
            from_schema: "dbo".into(),
            from_table: "Orders".into(),
            from_columns: vec!["CustomerId".into()],
            to_schema: "dbo".into(),
            to_table: "Customers".into(),
            to_columns: vec!["Id".into()],
        }],
    }
}

/// A small linear congruential generator: the same seed, the same run.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    /// A number in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// True `percent` times in a hundred.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}
