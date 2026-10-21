//! Estimated execution plans as a tree, with warnings about the usual
//! performance problems: scans that read a whole table to find a few rows,
//! key lookups, implicit conversions, missing join predicates and missing
//! indexes.
//!
//! Inputs are what the servers return: SQL Server showplan XML, Postgres
//! `EXPLAIN (FORMAT JSON)`, MySQL `EXPLAIN FORMAT=TREE` (or the classic
//! tabular EXPLAIN).

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    /// Operator, e.g. `Clustered Index Seek`, `Seq Scan`, `Table scan`.
    pub op: String,
    /// Table / index it works on.
    pub object: Option<String>,
    /// Predicate, join condition or other detail.
    pub detail: Option<String>,
    /// Estimated rows produced.
    pub rows: Option<f64>,
    /// Estimated cost of this node and everything below it.
    pub cost: Option<f64>,
    /// Share of the statement cost spent in this node itself (0–100).
    pub cost_percent: Option<f64>,
    pub warnings: Vec<String>,
    pub children: Vec<PlanNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingIndex {
    pub table: String,
    /// Estimated improvement in percent (SQL Server).
    pub impact: Option<f64>,
    pub create_statement: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementPlan {
    pub statement: String,
    pub cost: Option<f64>,
    pub root: Option<PlanNode>,
    /// Statement-level warnings plus a summary of the node warnings.
    pub warnings: Vec<String>,
    pub missing_indexes: Vec<MissingIndex>,
}

/// Rows read by a scan before it is worth warning about.
const SCAN_WARNING_ROWS: f64 = 1000.0;

fn fmt_rows(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 10_000.0 {
        format!("{:.0}k", n / 1000.0)
    } else {
        format!("{n:.0}")
    }
}

/// Fill in `cost_percent` and collect node warnings into the statement.
fn finish(mut plan: StatementPlan) -> StatementPlan {
    fn walk(n: &mut PlanNode, total: f64, out: &mut Vec<String>) {
        let below: f64 = n.children.iter().filter_map(|c| c.cost).sum();
        if let Some(c) = n.cost
            && total > 0.0
        {
            n.cost_percent = Some(((c - below).max(0.0) / total * 100.0 * 10.0).round() / 10.0);
        }
        for w in &n.warnings {
            if !out.contains(w) {
                out.push(w.clone());
            }
        }
        for c in &mut n.children {
            walk(c, total, out);
        }
    }
    let total = plan
        .cost
        .or_else(|| plan.root.as_ref().and_then(|r| r.cost))
        .unwrap_or(0.0);
    plan.cost = plan.cost.or(Some(total)).filter(|c| *c > 0.0);
    let mut found = Vec::new();
    if let Some(root) = plan.root.as_mut() {
        walk(root, total, &mut found);
    }
    for w in found {
        if !plan.warnings.contains(&w) {
            plan.warnings.push(w);
        }
    }
    plan
}

// ---------------------------------------------------------------------------
// SQL Server showplan XML
// ---------------------------------------------------------------------------

fn attr_f64(n: roxmltree::Node<'_, '_>, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|v| v.parse().ok())
}

fn is(n: roxmltree::Node<'_, '_>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

/// Descendants of `n` named `name` that belong to `n` itself, not to a
/// nested RelOp.
fn own_descendants<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    let mut out = Vec::new();
    let mut stack: Vec<_> = n.children().collect();
    stack.reverse();
    while let Some(c) = stack.pop() {
        if !c.is_element() {
            continue;
        }
        if is(c, name) {
            out.push(c);
        }
        if !is(c, "RelOp") {
            let mut kids: Vec<_> = c.children().collect();
            kids.reverse();
            stack.extend(kids);
        }
    }
    out
}

fn unbracket(s: &str) -> String {
    s.trim_matches(|c| c == '[' || c == ']').to_string()
}

fn relop(n: roxmltree::Node<'_, '_>) -> PlanNode {
    let physical = n.attribute("PhysicalOp").unwrap_or("?");
    let logical = n.attribute("LogicalOp").unwrap_or("");
    let op = if logical.is_empty() || logical == physical || physical.contains(logical) {
        physical.to_string()
    } else {
        format!("{physical} ({logical})")
    };
    let object = own_descendants(n, "Object").into_iter().next().map(|o| {
        let table = format!(
            "{}{}",
            o.attribute("Schema")
                .map(|s| format!("{}.", unbracket(s)))
                .unwrap_or_default(),
            unbracket(o.attribute("Table").unwrap_or("?"))
        );
        match o.attribute("Index") {
            Some(ix) => format!("{table} ({})", unbracket(ix)),
            None => table,
        }
    });
    let scalar = |parent: &str| {
        own_descendants(n, parent)
            .into_iter()
            .flat_map(|p| p.descendants().filter(|d| is(*d, "ScalarOperator")).take(1))
            .filter_map(|s| s.attribute("ScalarString").map(str::to_string))
            .next()
    };
    let predicate = scalar("Predicate");
    let detail = predicate
        .clone()
        .or_else(|| scalar("SeekPredicates"))
        .or_else(|| scalar("ProbeResidual"));
    let rows = attr_f64(n, "EstimateRows");
    let mut node = PlanNode {
        op,
        object: object.clone(),
        detail,
        rows,
        cost: attr_f64(n, "EstimatedTotalSubtreeCost"),
        ..Default::default()
    };

    let table = object.as_deref().unwrap_or("the table");
    let read = attr_f64(n, "TableCardinality").or_else(|| attr_f64(n, "EstimatedRowsRead"));
    if matches!(physical, "Table Scan" | "Clustered Index Scan" | "Index Scan")
        && predicate.is_some()
        && let Some(read) = read.filter(|r| *r >= SCAN_WARNING_ROWS)
    {
        node.warnings.push(format!(
            "{physical} of {table} reads {} rows to return about {}: an index on the filtered columns would let it seek",
            fmt_rows(read),
            fmt_rows(rows.unwrap_or(0.0))
        ));
    }
    if matches!(physical, "Key Lookup" | "RID Lookup") {
        node.warnings.push(format!(
            "{physical} on {table} fetches each row separately: add the columns it reads to the index (INCLUDE)"
        ));
    }
    for w in own_descendants(n, "Warnings") {
        if w.attribute("NoJoinPredicate") == Some("true") || w.children().any(|c| is(c, "NoJoinPredicate")) {
            node.warnings
                .push("No join predicate: every row is joined with every row of the other input".into());
        }
        for c in w.children().filter(|c| is(*c, "ColumnsWithNoStatistics")) {
            let cols: Vec<String> = c
                .descendants()
                .filter(|d| is(*d, "ColumnReference"))
                .filter_map(|d| d.attribute("Column").map(unbracket))
                .collect();
            node.warnings
                .push(format!("Columns without statistics: {}", cols.join(", ")));
        }
    }
    node.children = own_descendants(n, "RelOp").into_iter().map(relop).collect();
    node
}

fn missing_indexes(plan: roxmltree::Node<'_, '_>) -> Vec<MissingIndex> {
    let mut out = Vec::new();
    for group in plan.descendants().filter(|d| is(*d, "MissingIndexGroup")) {
        let impact = attr_f64(group, "Impact");
        for mi in group.children().filter(|c| is(*c, "MissingIndex")) {
            let schema = mi.attribute("Schema").unwrap_or("[dbo]");
            let table = mi.attribute("Table").unwrap_or("?");
            let cols = |usage: &str| -> Vec<String> {
                mi.children()
                    .filter(|c| is(*c, "ColumnGroup") && c.attribute("Usage") == Some(usage))
                    .flat_map(|c| c.children().filter(|x| is(*x, "Column")))
                    .filter_map(|c| c.attribute("Name").map(str::to_string))
                    .collect()
            };
            let mut keys = cols("EQUALITY");
            keys.extend(cols("INEQUALITY"));
            let include = cols("INCLUDE");
            let name = format!(
                "IX_{}_{}",
                unbracket(table),
                keys.iter().map(|k| unbracket(k)).collect::<Vec<_>>().join("_")
            );
            let mut create = format!(
                "CREATE NONCLUSTERED INDEX [{name}] ON {schema}.{table} ({})",
                keys.join(", ")
            );
            if !include.is_empty() {
                create.push_str(&format!(" INCLUDE ({})", include.join(", ")));
            }
            create.push(';');
            out.push(MissingIndex {
                table: format!("{}.{}", unbracket(schema), unbracket(table)),
                impact,
                create_statement: create,
            });
        }
    }
    out
}

/// Parse `SET SHOWPLAN_XML ON` output: one plan per statement.
pub fn parse_showplan_xml(xml: &str) -> Result<Vec<StatementPlan>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("invalid showplan XML: {e}"))?;
    let mut out = Vec::new();
    for stmt in doc.descendants().filter(|d| is(*d, "StmtSimple")) {
        let Some(qp) = stmt.children().find(|c| is(*c, "QueryPlan")) else {
            continue;
        };
        let mut warnings = Vec::new();
        if let Some(w) = qp.children().find(|c| is(*c, "Warnings")) {
            for c in w.children().filter(|c| is(*c, "PlanAffectingConvert")) {
                if c.attribute("ConvertIssue") == Some("Seek Plan") {
                    warnings.push(format!(
                        "Implicit conversion {} can prevent an index seek: compare values of the column's own type",
                        c.attribute("Expression").unwrap_or("")
                    ));
                }
            }
        }
        let plan = StatementPlan {
            statement: stmt.attribute("StatementText").unwrap_or("").trim().to_string(),
            cost: attr_f64(stmt, "StatementSubTreeCost"),
            root: qp.children().find(|c| is(*c, "RelOp")).map(relop),
            warnings,
            missing_indexes: missing_indexes(qp),
        };
        out.push(finish(plan));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Postgres EXPLAIN (FORMAT JSON)
// ---------------------------------------------------------------------------

fn pg_node(v: &serde_json::Value) -> PlanNode {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64());
    let node_type = s("Node Type").unwrap_or_else(|| "?".into());
    let op = match s("Join Type").filter(|j| j != "Inner") {
        Some(j) => format!("{node_type} ({j})"),
        None => node_type.clone(),
    };
    let object = s("Relation Name").map(|r| {
        let table = match s("Schema") {
            Some(sc) => format!("{sc}.{r}"),
            None => r,
        };
        match s("Index Name") {
            Some(ix) => format!("{table} ({ix})"),
            None => table,
        }
    });
    let filter = s("Filter");
    let detail = ["Index Cond", "Hash Cond", "Merge Cond", "Join Filter", "Recheck Cond"]
        .iter()
        .find_map(|k| s(k))
        .map(|c| match &filter {
            Some(f) => format!("{c}; filter {f}"),
            None => c,
        })
        .or_else(|| filter.clone())
        .or_else(|| {
            v.get("Sort Key")
                .and_then(|k| k.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
        });
    let mut node = PlanNode {
        op,
        object: object.clone(),
        detail,
        rows: f("Plan Rows"),
        cost: f("Total Cost"),
        ..Default::default()
    };
    if node_type == "Seq Scan" && filter.is_some() {
        node.warnings.push(format!(
            "Seq Scan of {} reads the whole table to apply the filter: an index on the filtered columns may help",
            object.as_deref().unwrap_or("the table")
        ));
    }
    node.children = v
        .get("Plans")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().map(pg_node).collect())
        .unwrap_or_default();
    node
}

/// Parse the output of `EXPLAIN (FORMAT JSON) <statement>`.
pub fn parse_pg_json(statement: &str, json: &str) -> Result<StatementPlan, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("invalid EXPLAIN output: {e}"))?;
    let plan = v.get(0).and_then(|x| x.get("Plan")).ok_or("EXPLAIN returned no plan")?;
    let root = pg_node(plan);
    Ok(finish(StatementPlan {
        statement: statement.trim().to_string(),
        cost: root.cost,
        root: Some(root),
        ..Default::default()
    }))
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

fn mysql_tree_node(text: &str) -> PlanNode {
    // "Table scan on o  (cost=0.55 rows=3)"
    let (body, stats) = match text.rfind("  (") {
        Some(i) => (text[..i].trim(), &text[i..]),
        None => (text.trim(), ""),
    };
    let stat = |key: &str| -> Option<f64> {
        let at = stats.find(&format!("{key}="))? + key.len() + 1;
        let rest = &stats[at..];
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '+' || c == '-'))
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    };
    let mut node = PlanNode {
        rows: stat("rows"),
        cost: stat("cost"),
        ..Default::default()
    };
    if let Some((op, rest)) = body.split_once(": ") {
        node.op = op.to_string();
        node.detail = Some(rest.to_string());
    } else if let Some((op, rest)) = body.split_once(" on ") {
        node.op = op.to_string();
        match rest.split_once(" using ") {
            Some((obj, using)) => {
                node.object = Some(obj.to_string());
                node.detail = Some(format!("using {using}"));
            }
            None => node.object = Some(rest.to_string()),
        }
    } else {
        node.op = body.to_string();
    }
    let lower = node.op.to_lowercase();
    if lower.starts_with("table scan") && node.rows.is_some_and(|r| r >= 1.0) {
        node.warnings.push(format!(
            "Full table scan of {}: an index on the columns it is filtered or joined by may help",
            node.object.as_deref().unwrap_or("the table")
        ));
    }
    node
}

/// Parse `EXPLAIN FORMAT=TREE` output (MySQL 8).
pub fn parse_mysql_tree(statement: &str, text: &str) -> StatementPlan {
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut root: Option<PlanNode> = None;
    let attach =
        |stack: &mut Vec<(usize, PlanNode)>, root: &mut Option<PlanNode>, node: PlanNode| match stack.last_mut() {
            Some((_, parent)) => parent.children.push(node),
            None => *root = Some(node),
        };
    for line in text.lines() {
        let Some(at) = line.find("-> ") else { continue };
        let indent = at;
        let node = mysql_tree_node(&line[at + 3..]);
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            let (_, done) = stack.pop().expect("non-empty");
            attach(&mut stack, &mut root, done);
        }
        stack.push((indent, node));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut stack, &mut root, done);
    }
    finish(StatementPlan {
        statement: statement.trim().to_string(),
        cost: root.as_ref().and_then(|r| r.cost),
        root,
        ..Default::default()
    })
}

/// Classic tabular `EXPLAIN` (MariaDB, MySQL 5.x): one node per table.
pub fn parse_mysql_table(statement: &str, columns: &[String], rows: &[Vec<Option<String>>]) -> StatementPlan {
    let idx = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let get = |r: &Vec<Option<String>>, name: &str| idx(name).and_then(|i| r.get(i).cloned().flatten());
    let children: Vec<PlanNode> = rows
        .iter()
        .map(|r| {
            let access = get(r, "type").unwrap_or_default();
            let table = get(r, "table");
            let mut node = PlanNode {
                op: match access.as_str() {
                    "ALL" => "Full table scan".to_string(),
                    "index" => "Full index scan".to_string(),
                    other => format!("{other} access"),
                },
                object: table.clone(),
                detail: [get(r, "key").map(|k| format!("key {k}")), get(r, "Extra")]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ")
                    .into(),
                rows: get(r, "rows").and_then(|x| x.parse().ok()),
                ..Default::default()
            };
            if node.detail.as_deref() == Some("") {
                node.detail = None;
            }
            if access == "ALL" {
                node.warnings.push(format!(
                    "Full table scan of {}: an index on the columns it is filtered or joined by may help",
                    table.as_deref().unwrap_or("the table")
                ));
            }
            node
        })
        .collect();
    let root = PlanNode {
        op: "Query".into(),
        children,
        ..Default::default()
    };
    finish(StatementPlan {
        statement: statement.trim().to_string(),
        root: Some(root),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWPLAN: &str = r#"<?xml version="1.0" encoding="utf-16"?>
<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.5">
 <BatchSequence><Batch><Statements>
  <StmtSimple StatementText="SELECT Id, Name FROM dbo.Customers WHERE Code = N'A1'" StatementSubTreeCost="0.5">
   <QueryPlan>
    <Warnings><PlanAffectingConvert ConvertIssue="Seek Plan" Expression="CONVERT_IMPLICIT(nvarchar(10),[c].[Code],0)=N'A1'" /></Warnings>
    <MissingIndexes><MissingIndexGroup Impact="92.5"><MissingIndex Database="[Shop]" Schema="[dbo]" Table="[Customers]">
      <ColumnGroup Usage="EQUALITY"><Column Name="[Code]" ColumnId="2" /></ColumnGroup>
      <ColumnGroup Usage="INCLUDE"><Column Name="[Name]" ColumnId="3" /></ColumnGroup>
    </MissingIndex></MissingIndexGroup></MissingIndexes>
    <RelOp NodeId="0" PhysicalOp="Nested Loops" LogicalOp="Inner Join" EstimateRows="10" EstimatedTotalSubtreeCost="0.5">
     <NestedLoops>
      <RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="10" EstimatedTotalSubtreeCost="0.4" TableCardinality="50000">
       <IndexScan><Object Database="[Shop]" Schema="[dbo]" Table="[Customers]" Index="[PK_Customers]" />
        <Predicate><ScalarOperator ScalarString="[c].[Code]=N'A1'" /></Predicate>
       </IndexScan>
      </RelOp>
      <RelOp NodeId="2" PhysicalOp="Key Lookup" LogicalOp="Key Lookup" EstimateRows="1" EstimatedTotalSubtreeCost="0.05">
       <IndexScan><Object Database="[Shop]" Schema="[dbo]" Table="[Customers]" Index="[PK_Customers]" /></IndexScan>
      </RelOp>
     </NestedLoops>
    </RelOp>
   </QueryPlan>
  </StmtSimple>
 </Statements></Batch></BatchSequence>
</ShowPlanXML>"#;

    #[test]
    fn sql_server_plan_tree_and_warnings() {
        let plans = parse_showplan_xml(SHOWPLAN).unwrap();
        assert_eq!(plans.len(), 1);
        let p = &plans[0];
        let root = p.root.as_ref().unwrap();
        assert_eq!(root.op, "Nested Loops (Inner Join)");
        assert_eq!(root.children.len(), 2);
        let scan = &root.children[0];
        assert_eq!(scan.object.as_deref(), Some("dbo.Customers (PK_Customers)"));
        assert_eq!(scan.detail.as_deref(), Some("[c].[Code]=N'A1'"));
        assert_eq!(scan.cost_percent, Some(80.0));
        assert!(scan.warnings[0].contains("reads 50k rows"), "{:?}", scan.warnings);
        assert!(root.children[1].warnings[0].starts_with("Key Lookup"));
        assert!(p.warnings.iter().any(|w| w.starts_with("Implicit conversion")));
        assert_eq!(p.warnings.len(), 3, "{:?}", p.warnings);
        assert_eq!(
            p.missing_indexes[0].create_statement,
            "CREATE NONCLUSTERED INDEX [IX_Customers_Code] ON [dbo].[Customers] ([Code]) INCLUDE ([Name]);"
        );
        assert_eq!(p.missing_indexes[0].impact, Some(92.5));
    }

    #[test]
    fn postgres_plan() {
        let json = r#"[{"Plan": {"Node Type": "Hash Join", "Join Type": "Inner", "Total Cost": 40.5, "Plan Rows": 12,
            "Hash Cond": "(o.customer_id = c.id)",
            "Plans": [
              {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "public", "Alias": "o", "Total Cost": 30.0, "Plan Rows": 12, "Filter": "(total > 100)"},
              {"Node Type": "Hash", "Total Cost": 5.0, "Plan Rows": 3, "Plans": [
                {"Node Type": "Index Scan", "Relation Name": "customers", "Schema": "public", "Index Name": "customers_pkey", "Total Cost": 5.0, "Plan Rows": 3}]}
            ]}}]"#;
        let p = parse_pg_json("SELECT 1", json).unwrap();
        let root = p.root.unwrap();
        assert_eq!(root.op, "Hash Join");
        assert_eq!(root.detail.as_deref(), Some("(o.customer_id = c.id)"));
        assert_eq!(root.children[0].object.as_deref(), Some("public.orders"));
        assert_eq!(
            root.children[1].children[0].object.as_deref(),
            Some("public.customers (customers_pkey)")
        );
        assert!(p.warnings[0].starts_with("Seq Scan of public.orders"));
    }

    #[test]
    fn mysql_tree_plan() {
        let text = "-> Nested loop inner join  (cost=1.60 rows=3)\n    -> Filter: (o.Total > 1)  (cost=0.55 rows=1)\n        -> Table scan on o  (cost=0.55 rows=3)\n    -> Single-row index lookup on c using PRIMARY (Id=o.CustomerId)  (cost=0.35 rows=1)\n";
        let p = parse_mysql_tree("SELECT 1", text);
        let root = p.root.unwrap();
        assert_eq!(root.op, "Nested loop inner join");
        assert_eq!(root.cost, Some(1.6));
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].op, "Filter");
        assert_eq!(root.children[0].children[0].object.as_deref(), Some("o"));
        assert_eq!(
            root.children[1].detail.as_deref(),
            Some("using PRIMARY (Id=o.CustomerId)")
        );
        assert!(p.warnings[0].starts_with("Full table scan of o"));
    }

    #[test]
    fn mysql_table_plan() {
        let cols: Vec<String> = ["id", "select_type", "table", "type", "key", "rows", "Extra"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = vec![vec![
            Some("1".into()),
            Some("SIMPLE".into()),
            Some("t".into()),
            Some("ALL".into()),
            None,
            Some("20".into()),
            Some("Using where".into()),
        ]];
        let p = parse_mysql_table("SELECT 1", &cols, &rows);
        assert_eq!(p.root.as_ref().unwrap().children[0].op, "Full table scan");
        assert_eq!(p.warnings.len(), 1);
    }
}
