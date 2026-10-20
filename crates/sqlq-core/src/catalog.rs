//! Database metadata model used by completion. Filled by `sqlq-db`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    Procedure,
    ScalarFunction,
    TableFunction,
    Trigger,
    Synonym,
    Sequence,
}

impl ObjectKind {
    /// Can appear after FROM / JOIN.
    pub fn is_relation(self) -> bool {
        matches!(
            self,
            ObjectKind::Table | ObjectKind::View | ObjectKind::TableFunction | ObjectKind::Synonym
        )
    }

    pub fn is_routine(self) -> bool {
        matches!(
            self,
            ObjectKind::Procedure | ObjectKind::ScalarFunction | ObjectKind::TableFunction
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub is_primary_key: bool,
    #[serde(default)]
    pub is_identity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    pub name: String,
    pub data_type: String,
    #[serde(default)]
    pub is_output: bool,
    #[serde(default)]
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbObject {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    pub from_schema: String,
    pub from_table: String,
    pub from_columns: Vec<String>,
    pub to_schema: String,
    pub to_table: String,
    pub to_columns: Vec<String>,
}

/// Source text of a programmable object (view, procedure, function, trigger).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectDefinition {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    pub definition: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub database: String,
    pub default_schema: String,
    pub schemas: Vec<String>,
    pub objects: Vec<DbObject>,
    pub foreign_keys: Vec<ForeignKey>,
}

impl Catalog {
    pub fn is_schema(&self, name: &str) -> bool {
        self.schemas.iter().any(|s| s.eq_ignore_ascii_case(name))
    }

    /// Find an object by optional schema and name (case-insensitive). Without
    /// a schema the default schema wins over other matches.
    pub fn find(&self, schema: Option<&str>, name: &str) -> Option<&DbObject> {
        let mut candidates = self.objects.iter().filter(|o| {
            o.name.eq_ignore_ascii_case(name)
                && schema.is_none_or(|s| o.schema.eq_ignore_ascii_case(s))
        });
        let first = candidates.next()?;
        if schema.is_some() || first.schema.eq_ignore_ascii_case(&self.default_schema) {
            return Some(first);
        }
        candidates
            .find(|o| o.schema.eq_ignore_ascii_case(&self.default_schema))
            .or(Some(first))
    }

    /// Foreign keys connecting the two tables in either direction.
    pub fn foreign_keys_between<'a>(
        &'a self,
        a: &'a DbObject,
        b: &'a DbObject,
    ) -> impl Iterator<Item = &'a ForeignKey> {
        self.foreign_keys.iter().filter(move |fk| {
            let from_a = fk.from_schema.eq_ignore_ascii_case(&a.schema)
                && fk.from_table.eq_ignore_ascii_case(&a.name);
            let to_a = fk.to_schema.eq_ignore_ascii_case(&a.schema)
                && fk.to_table.eq_ignore_ascii_case(&a.name);
            let from_b = fk.from_schema.eq_ignore_ascii_case(&b.schema)
                && fk.from_table.eq_ignore_ascii_case(&b.name);
            let to_b = fk.to_schema.eq_ignore_ascii_case(&b.schema)
                && fk.to_table.eq_ignore_ascii_case(&b.name);
            (from_a && to_b) || (from_b && to_a)
        })
    }
}
