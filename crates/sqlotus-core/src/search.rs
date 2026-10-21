//! SQL Search: find objects by name, column name or text of their definition.

use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, ObjectDefinition, ObjectKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchOptions {
    /// Match whole words only.
    pub exact_match: bool,
    pub case_sensitive: bool,
    /// Object kinds to include; empty means all.
    pub kinds: Vec<ObjectKind>,
    pub search_names: bool,
    pub search_columns: bool,
    pub search_definitions: bool,
    pub max_results: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            exact_match: false,
            case_sensitive: false,
            kinds: Vec::new(),
            search_names: true,
            search_columns: true,
            search_definitions: true,
            max_results: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchIn {
    Name,
    Column,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    pub matched_in: MatchIn,
    /// Column name, or the trimmed first matching line of the definition.
    pub detail: String,
    /// 1-based line of the first match in the definition (0 otherwise).
    pub line: usize,
    pub match_count: usize,
}

struct Matcher {
    needle: String,
    case_sensitive: bool,
    whole_word: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '@' || c == '#' || c == '$'
}

impl Matcher {
    fn new(query: &str, opts: &SearchOptions) -> Self {
        let needle = if opts.case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        Self {
            needle,
            case_sensitive: opts.case_sensitive,
            whole_word: opts.exact_match,
        }
    }

    /// Byte offsets (in the possibly lowercased haystack) of all matches.
    fn find_all(&self, hay: &str) -> (Vec<usize>, String) {
        let hay = if self.case_sensitive {
            hay.to_string()
        } else {
            hay.to_lowercase()
        };
        let mut hits = Vec::new();
        let mut from = 0;
        while let Some(pos) = hay[from..].find(&self.needle) {
            let at = from + pos;
            let end = at + self.needle.len();
            let ok = !self.whole_word
                || (hay[..at].chars().next_back().is_none_or(|c| !is_word_char(c))
                    && hay[end..].chars().next().is_none_or(|c| !is_word_char(c)));
            if ok {
                hits.push(at);
            }
            from = at + self.needle.len().max(1);
            while !hay.is_char_boundary(from) {
                from += 1;
            }
        }
        (hits, hay)
    }

    fn matches(&self, hay: &str) -> bool {
        !self.find_all(hay).0.is_empty()
    }
}

pub fn search(
    query: &str,
    database: &str,
    catalog: &Catalog,
    definitions: &[ObjectDefinition],
    opts: &SearchOptions,
) -> Vec<SearchHit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let m = Matcher::new(query, opts);
    let kind_ok = |k: ObjectKind| opts.kinds.is_empty() || opts.kinds.contains(&k);
    let mut hits = Vec::new();

    for o in catalog.objects.iter().filter(|o| kind_ok(o.kind)) {
        if opts.search_names && (m.matches(&o.name) || m.matches(&format!("{}.{}", o.schema, o.name))) {
            hits.push(SearchHit {
                database: database.to_string(),
                schema: o.schema.clone(),
                name: o.name.clone(),
                kind: o.kind,
                matched_in: MatchIn::Name,
                detail: String::new(),
                line: 0,
                match_count: 1,
            });
        }
        if opts.search_columns {
            for c in o.columns.iter().filter(|c| m.matches(&c.name)) {
                hits.push(SearchHit {
                    database: database.to_string(),
                    schema: o.schema.clone(),
                    name: o.name.clone(),
                    kind: o.kind,
                    matched_in: MatchIn::Column,
                    detail: format!("{} {}", c.name, c.data_type),
                    line: 0,
                    match_count: 1,
                });
            }
        }
    }

    if opts.search_definitions {
        for d in definitions.iter().filter(|d| kind_ok(d.kind)) {
            let (positions, hay) = m.find_all(&d.definition);
            let Some(&first) = positions.first() else {
                continue;
            };
            let line_start = hay[..first].rfind('\n').map_or(0, |i| i + 1);
            let line_no = hay[..first].matches('\n').count() + 1;
            // take the line from the original text (same line numbering)
            let line_text = d
                .definition
                .lines()
                .nth(line_no - 1)
                .unwrap_or(&hay[line_start..])
                .trim();
            hits.push(SearchHit {
                database: database.to_string(),
                schema: d.schema.clone(),
                name: d.name.clone(),
                kind: d.kind,
                matched_in: MatchIn::Definition,
                detail: line_text.chars().take(200).collect(),
                line: line_no,
                match_count: positions.len(),
            });
        }
    }

    hits.sort_by(|a, b| {
        a.matched_in
            .cmp(&b.matched_in)
            .then_with(|| a.schema.to_lowercase().cmp(&b.schema.to_lowercase()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    hits.truncate(opts.max_results.max(1));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, DbObject};

    fn data() -> (Catalog, Vec<ObjectDefinition>) {
        let cat = Catalog {
            objects: vec![
                DbObject {
                    schema: "dbo".into(),
                    name: "Customers".into(),
                    kind: ObjectKind::Table,
                    columns: vec![Column {
                        name: "CustomerId".into(),
                        data_type: "int".into(),
                        nullable: false,
                        is_primary_key: true,
                        is_identity: true,
                        is_computed: false,
                    }],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "GetCustomer".into(),
                    kind: ObjectKind::Procedure,
                    columns: vec![],
                    params: vec![],
                },
            ],
            ..Default::default()
        };
        let defs = vec![ObjectDefinition {
            schema: "dbo".into(),
            name: "GetCustomer".into(),
            kind: ObjectKind::Procedure,
            definition: "CREATE PROCEDURE dbo.GetCustomer\nAS\nSELECT * FROM dbo.Customers WHERE CustomerId = 1 OR customerid = 2".into(),
        }];
        (cat, defs)
    }

    #[test]
    fn finds_names_columns_and_text() {
        let (cat, defs) = data();
        let hits = search("customerid", "Shop", &cat, &defs, &SearchOptions::default());
        let got: Vec<_> = hits
            .iter()
            .map(|h| (h.matched_in, h.name.as_str(), h.line, h.match_count))
            .collect();
        assert_eq!(
            got,
            vec![
                (MatchIn::Column, "Customers", 0, 1),
                (MatchIn::Definition, "GetCustomer", 3, 2)
            ]
        );
        assert!(hits[1].detail.starts_with("SELECT * FROM"));
    }

    #[test]
    fn whole_word_and_case() {
        let (cat, defs) = data();
        let opts = SearchOptions {
            exact_match: true,
            ..Default::default()
        };
        let hits = search("Customer", "Shop", &cat, &defs, &opts);
        assert!(hits.is_empty(), "{hits:?}");
        let opts = SearchOptions {
            case_sensitive: true,
            search_names: false,
            search_columns: false,
            ..Default::default()
        };
        let hits = search("customerid", "Shop", &cat, &defs, &opts);
        assert_eq!(hits[0].match_count, 1);
    }

    #[test]
    fn kind_filter() {
        let (cat, defs) = data();
        let opts = SearchOptions {
            kinds: vec![ObjectKind::Table],
            ..Default::default()
        };
        let hits = search("Customer", "Shop", &cat, &defs, &opts);
        assert!(hits.iter().all(|h| h.kind == ObjectKind::Table));
        assert_eq!(hits.len(), 2);
    }
}
