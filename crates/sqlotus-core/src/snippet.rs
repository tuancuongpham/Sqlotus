//! Snippets stored in Redgate SQL Prompt's JSON shape, converted to
//! Monaco/VS Code snippet syntax for insertion.
//!
//! Redgate bodies use `$name$` placeholders plus built-ins such as
//! `$CURSOR$`, `$SELECTEDTEXT$`, `$SELECTIONSTART$`/`$SELECTIONEND$`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placeholder {
    pub name: String,
    #[serde(default)]
    pub default_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    #[serde(default)]
    pub id: String,
    pub prefix: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub placeholders: Vec<Placeholder>,
}

/// Escape literal text for snippet syntax.
pub fn escape_snippet(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '$' | '}') {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// Convert to Monaco snippet syntax. `vars` supplies values for `$USER$`,
/// `$MACHINE$`, `$DBNAME$` and `$SERVER$`.
pub fn to_monaco(snippet: &Snippet, vars: &HashMap<String, String>) -> String {
    let body = &snippet.body;
    let mut out = String::new();
    let mut indices: HashMap<String, usize> = HashMap::new();
    let mut next_index = 1usize;
    let mut selection: Option<(usize, String)> = None; // (index, collected literal)
    let mut rest = body.as_str();

    let push_lit =
        |out: &mut String, selection: &mut Option<(usize, String)>, text: &str| match selection {
            Some((_, buf)) => buf.push_str(text),
            None => out.push_str(&escape_snippet(text)),
        };

    while let Some(open) = rest.find('$') {
        let after = &rest[open + 1..];
        let close = after.find('$');
        let name = close
            .map(|c| &after[..c])
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let Some(name) = name else {
            push_lit(&mut out, &mut selection, &rest[..open + 1]);
            rest = after;
            continue;
        };
        push_lit(&mut out, &mut selection, &rest[..open]);
        rest = &after[name.len() + 1..];

        let upper = name.to_ascii_uppercase();
        let replacement = match upper.as_str() {
            "CURSOR" => Some("$0".to_string()),
            "SELECTEDTEXT" => Some("${TM_SELECTED_TEXT}".to_string()),
            "PASTE" => Some("${CLIPBOARD}".to_string()),
            "DATE" => Some("${CURRENT_YEAR}-${CURRENT_MONTH}-${CURRENT_DATE}".to_string()),
            "TIME" => Some("${CURRENT_HOUR}:${CURRENT_MINUTE}:${CURRENT_SECOND}".to_string()),
            "GUID" => Some("${UUID}".to_string()),
            "USER" | "MACHINE" | "DBNAME" | "SERVER" => Some(escape_snippet(
                vars.get(&upper).map_or(name, |v| v.as_str()),
            )),
            "SELECTIONSTART" | "SELECTIONEND" => {
                match selection.take() {
                    Some((idx, text)) => {
                        out.push_str(&format!("${{{idx}:{}}}", escape_snippet(&text)))
                    }
                    None => {
                        selection = Some((next_index, String::new()));
                        next_index += 1;
                    }
                }
                None
            }
            _ => {
                let idx = *indices.entry(name.to_string()).or_insert_with(|| {
                    next_index += 1;
                    next_index - 1
                });
                let default = snippet
                    .placeholders
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.default_value.as_str())
                    .filter(|d| !d.is_empty())
                    .unwrap_or(name);
                Some(format!("${{{idx}:{}}}", escape_snippet(default)))
            }
        };
        if let Some(r) = replacement {
            match &mut selection {
                // placeholders inside a selection region keep their literal default
                Some((_, buf)) => buf.push_str(&r),
                None => out.push_str(&r),
            }
        }
    }
    push_lit(&mut out, &mut selection, rest);
    if let Some((idx, text)) = selection {
        out.push_str(&format!("${{{idx}:{}}}", escape_snippet(&text)));
    }
    out
}

/// A few defaults so a fresh install is useful before any import.
pub fn builtin_snippets() -> Vec<Snippet> {
    let s = |prefix: &str, description: &str, body: &str| Snippet {
        id: format!("builtin-{prefix}"),
        prefix: prefix.into(),
        description: description.into(),
        body: body.into(),
        placeholders: Vec::new(),
    };
    vec![
        s("ssf", "SELECT * FROM", "SELECT * FROM $CURSOR$"),
        s(
            "st100",
            "SELECT TOP 100 * FROM",
            "SELECT TOP 100 * FROM $CURSOR$",
        ),
        s(
            "scf",
            "SELECT COUNT(*) FROM",
            "SELECT COUNT(*) FROM $CURSOR$",
        ),
        s(
            "ii",
            "INSERT INTO",
            "INSERT INTO $table$ ($columns$)\nVALUES ($CURSOR$)",
        ),
        s("uu", "UPDATE", "UPDATE $table$\nSET $CURSOR$\nWHERE "),
        s("df", "DELETE FROM", "DELETE FROM $table$\nWHERE $CURSOR$"),
        s("be", "BEGIN END", "BEGIN\n    $SELECTEDTEXT$$CURSOR$\nEND"),
        s(
            "tc",
            "TRY CATCH",
            "BEGIN TRY\n    $SELECTEDTEXT$$CURSOR$\nEND TRY\nBEGIN CATCH\n    THROW;\nEND CATCH",
        ),
        s(
            "bt",
            "BEGIN TRANSACTION",
            "BEGIN TRANSACTION;\n\n$CURSOR$\n\nROLLBACK TRANSACTION;",
        ),
        s(
            "cte",
            "Common table expression",
            "WITH $name$ AS (\n    $CURSOR$\n)\nSELECT * FROM $name$",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(body: &str, ph: &[(&str, &str)]) -> Snippet {
        Snippet {
            id: String::new(),
            prefix: "x".into(),
            description: String::new(),
            body: body.into(),
            placeholders: ph
                .iter()
                .map(|(n, d)| Placeholder {
                    name: (*n).into(),
                    default_value: (*d).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn redgate_add_column() {
        let s = snip(
            "ALTER TABLE $table_name$ ADD [$column_name$] $SELECTIONEND$INT$SELECTIONSTART$ --NULL|NOT NULL",
            &[("table_name", "[schema].[table_name]"), ("column_name", "")],
        );
        assert_eq!(
            to_monaco(&s, &HashMap::new()),
            "ALTER TABLE ${1:[schema].[table_name]} ADD [${2:column_name}] ${3:INT} --NULL|NOT NULL"
        );
    }

    #[test]
    fn repeated_placeholder_shares_index_and_cursor() {
        let s = snip("WITH $n$ AS ($CURSOR$) SELECT * FROM $n$ -- costs $5", &[]);
        assert_eq!(
            to_monaco(&s, &HashMap::new()),
            "WITH ${1:n} AS ($0) SELECT * FROM ${1:n} -- costs \\$5"
        );
    }

    #[test]
    fn variables_from_context() {
        let s = snip("-- by $USER$ on $DBNAME$\n$SELECTEDTEXT$", &[]);
        let vars = HashMap::from([("USER".to_string(), "me".to_string())]);
        assert_eq!(
            to_monaco(&s, &vars),
            "-- by me on DBNAME\n${TM_SELECTED_TEXT}"
        );
    }
}
