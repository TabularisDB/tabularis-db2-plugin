//! Turns the rows Db2's `EXPLAIN` writes into its explain tables into the
//! plan tree Tabularis draws.
//!
//! `EXPLAIN_OPERATOR` lists the operators (`RETURN`, `TBSCAN`, `HSJOIN`...),
//! `EXPLAIN_STREAM` the rows flowing between them and from tables and
//! indexes into them, `EXPLAIN_PREDICATE` where each predicate is applied
//! and `EXPLAIN_ARGUMENT` the settings of each operator. Nothing here talks
//! to Db2: the handler reads the rows and hands them over.

use crate::models::ExplainNode;
use std::collections::{BTreeMap, HashMap};

/// One `EXPLAIN_OPERATOR` row.
#[derive(Debug, Clone, Default)]
pub struct Operator {
    pub id: i64,
    pub operator_type: String,
    pub total_cost: Option<f64>,
    pub io_cost: Option<f64>,
    pub cpu_cost: Option<f64>,
    pub first_row_cost: Option<f64>,
    pub re_total_cost: Option<f64>,
    pub buffers: Option<f64>,
}

/// Where a stream starts: another operator, or a table or index (a "data
/// object", source id -1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamSource {
    Operator(i64),
    Object { schema: String, name: String },
}

/// One `EXPLAIN_STREAM` row that ends in an operator. Streams that end in a
/// data object (the table an `INSERT` writes to) are not part of the tree.
#[derive(Debug, Clone)]
pub struct Stream {
    pub source: StreamSource,
    pub target: i64,
    /// Estimated number of rows.
    pub count: Option<f64>,
}

/// One `EXPLAIN_PREDICATE` row.
#[derive(Debug, Clone)]
pub struct Predicate {
    pub operator_id: i64,
    pub how_applied: String,
    pub text: String,
}

/// One `EXPLAIN_ARGUMENT` row.
#[derive(Debug, Clone)]
pub struct Argument {
    pub operator_id: i64,
    pub kind: String,
    pub value: String,
}

/// Everything `EXPLAIN` wrote about one statement.
#[derive(Debug, Default)]
pub struct PlanRows {
    pub operators: Vec<Operator>,
    pub streams: Vec<Stream>,
    pub predicates: Vec<Predicate>,
    pub arguments: Vec<Argument>,
    /// The table of each index the plan reads, keyed by the index's
    /// `(schema, name)`.
    pub index_tables: HashMap<(String, String), (String, String)>,
}

/// Names a table or index the way the plan shows it: by name alone in the
/// schema the statement ran in, qualified anywhere else.
fn object_label(schema: &str, name: &str, current_schema: &str) -> String {
    if schema == current_schema {
        name.to_string()
    } else {
        format!("{schema}.{name}")
    }
}

/// Joins predicates that apply the same way, once each: Db2 lists a range
/// predicate as both the start and the stop of an index scan when it is an
/// equality.
fn join_predicates<'a>(texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for text in texts {
        if !seen.contains(&text) {
            seen.push(text);
        }
    }
    (!seen.is_empty()).then(|| seen.join(" AND "))
}

/// The join type of a join operator, from its `OUTERJN` argument: Db2 names
/// the side whose rows are all kept, as seen from the operator's own outer
/// and inner input.
fn join_type(operator_type: &str, arguments: &BTreeMap<String, Vec<String>>) -> Option<String> {
    if !operator_type.ends_with("JOIN") {
        return None;
    }
    let outer_join = arguments
        .get("OUTERJN")
        .and_then(|values| values.first())
        .map(|value| value.trim().to_uppercase());
    Some(match outer_join.as_deref() {
        None | Some("") => "Inner".to_string(),
        Some("LEFT") => "Left".to_string(),
        Some("RIGHT") => "Right".to_string(),
        Some("FULL") => "Full".to_string(),
        Some(other) => other.to_string(),
    })
}

/// Reads `PREPTIME` ("46 milliseconds"), the time Db2 took to compile the
/// statement.
pub fn planning_time_ms(rows: &PlanRows) -> Option<f64> {
    rows.arguments
        .iter()
        .find(|argument| argument.kind == "PREPTIME")
        .and_then(|argument| {
            let value = argument.value.trim();
            let number = value.split_whitespace().next()?.parse::<f64>().ok()?;
            match value.split_whitespace().nth(1) {
                Some(unit) if unit.starts_with("milli") => Some(number),
                Some(unit) if unit.starts_with("micro") => Some(number / 1000.0),
                Some(unit) if unit.starts_with("second") => Some(number * 1000.0),
                _ => None,
            }
        })
}

/// One operator as a plan node, without its children.
fn operator_node(operator: &Operator, rows: &PlanRows, current_schema: &str) -> ExplainNode {
    let id = operator.id;
    let mut node = ExplainNode::new(id.to_string(), operator.operator_type.trim());

    let mut arguments: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for argument in rows.arguments.iter().filter(|a| a.operator_id == id) {
        arguments
            .entry(argument.kind.trim().to_string())
            .or_default()
            .push(argument.value.trim().to_string());
    }

    // The table this operator reads, directly or through an index.
    for stream in rows.streams.iter().filter(|stream| stream.target == id) {
        let StreamSource::Object { schema, name } = &stream.source else {
            continue;
        };
        match rows.index_tables.get(&(schema.clone(), name.clone())) {
            Some((table_schema, table_name)) => {
                node.relation = Some(object_label(table_schema, table_name, current_schema));
                node.extra.insert(
                    "index".to_string(),
                    serde_json::json!(object_label(schema, name, current_schema)),
                );
            }
            None => {
                node.relation = Some(object_label(schema, name, current_schema));
                if node.node_type == "TBSCAN" {
                    // Read by the explain view as a full scan of a table.
                    node.extra
                        .insert("access_type".to_string(), serde_json::json!("ALL"));
                }
            }
        }
    }

    // Estimated rows: what this operator passes on, or for the top
    // operator, which passes nothing on, what it receives.
    let output = rows
        .streams
        .iter()
        .find(|stream| stream.source == StreamSource::Operator(id))
        .and_then(|stream| stream.count);
    node.plan_rows = output.or_else(|| {
        rows.streams
            .iter()
            .filter(|stream| {
                stream.target == id && matches!(stream.source, StreamSource::Operator(_))
            })
            .filter_map(|stream| stream.count)
            .reduce(f64::max)
    });

    node.startup_cost = operator.first_row_cost;
    node.total_cost = operator.total_cost;

    let predicates: Vec<&Predicate> = rows
        .predicates
        .iter()
        .filter(|predicate| predicate.operator_id == id)
        .collect();
    let applied = |kinds: &[&str]| {
        join_predicates(
            predicates
                .iter()
                .filter(|predicate| kinds.contains(&predicate.how_applied.trim()))
                .map(|predicate| predicate.text.trim()),
        )
    };
    node.index_condition = applied(&["START", "STOP"]);
    node.filter = applied(&["SARG", "RESID"]);
    let join_condition = applied(&["JOIN"]);
    if node.node_type == "HSJOIN" {
        node.hash_condition = join_condition;
    } else if let Some(condition) = join_condition {
        node.extra
            .insert("join_condition".to_string(), serde_json::json!(condition));
    }
    for predicate in predicates.iter().filter(|predicate| {
        !["START", "STOP", "SARG", "RESID", "JOIN"].contains(&predicate.how_applied.trim())
    }) {
        node.extra.insert(
            format!("{} predicate", predicate.how_applied.trim().to_lowercase()),
            serde_json::json!(predicate.text.trim()),
        );
    }

    node.join_type = join_type(&node.node_type, &arguments);

    for (key, value) in [
        ("io_cost", operator.io_cost),
        ("cpu_cost", operator.cpu_cost),
        ("re_total_cost", operator.re_total_cost),
        ("buffers", operator.buffers),
    ] {
        if let Some(value) = value {
            node.extra.insert(key.to_string(), serde_json::json!(value));
        }
    }
    for (kind, values) in arguments {
        node.extra
            .insert(kind, serde_json::json!(values.join(", ")));
    }
    node
}

/// Order of an operator's inputs: the outer input of a join first, as
/// `db2exfmt` draws it, then by operator number.
fn input_order(rows: &PlanRows, source: i64) -> (u8, i64) {
    let side = rows
        .arguments
        .iter()
        .find(|argument| argument.operator_id == source && argument.kind.trim() == "JN INPUT")
        .map(|argument| match argument.value.trim() {
            "OUTER" => 0,
            "INNER" => 1,
            _ => 2,
        })
        .unwrap_or(2);
    (side, source)
}

fn build_node(
    operator: &Operator,
    rows: &PlanRows,
    by_id: &HashMap<i64, &Operator>,
    current_schema: &str,
    path: &mut Vec<i64>,
) -> ExplainNode {
    let mut node = operator_node(operator, rows, current_schema);
    let mut inputs: Vec<i64> = rows
        .streams
        .iter()
        .filter(|stream| stream.target == operator.id)
        .filter_map(|stream| match stream.source {
            StreamSource::Operator(source) => Some(source),
            StreamSource::Object { .. } => None,
        })
        .collect();
    inputs.sort_by_key(|source| input_order(rows, *source));
    inputs.dedup();

    path.push(operator.id);
    for source in inputs {
        // A temporary table read by two operators feeds both; it is drawn
        // under each. A loop would be a corrupt plan: stop there.
        if path.contains(&source) {
            continue;
        }
        if let Some(input) = by_id.get(&source) {
            node.children
                .push(build_node(input, rows, by_id, current_schema, path));
        }
    }
    path.pop();
    node
}

/// The plan tree, rooted at the operator that passes its rows to no other
/// one (`RETURN` for a query, `INSERT`, `UPDATE` or `DELETE` for a change).
pub fn build_plan_tree(rows: &PlanRows, current_schema: &str) -> Option<ExplainNode> {
    let by_id: HashMap<i64, &Operator> = rows
        .operators
        .iter()
        .map(|operator| (operator.id, operator))
        .collect();
    let has_consumer = |id: i64| {
        rows.streams.iter().any(|stream| {
            stream.source == StreamSource::Operator(id) && by_id.contains_key(&stream.target)
        })
    };
    let root = rows
        .operators
        .iter()
        .filter(|operator| !has_consumer(operator.id))
        .min_by_key(|operator| operator.id)?;
    Some(build_node(
        root,
        rows,
        &by_id,
        current_schema,
        &mut Vec::new(),
    ))
}

/// The plan as indented text, one operator per line, for the raw view.
pub fn plan_text(node: &ExplainNode) -> String {
    fn write(node: &ExplainNode, depth: usize, out: &mut String) {
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{} {}", node.id, node.node_type));
        if let Some(relation) = &node.relation {
            out.push_str(&format!(" on {relation}"));
        }
        if let Some(index) = node.extra.get("index").and_then(|value| value.as_str()) {
            out.push_str(&format!(" using {index}"));
        }
        if let Some(cost) = node.total_cost {
            out.push_str(&format!(" (cost={cost:.2}"));
            if let Some(rows) = node.plan_rows {
                out.push_str(&format!(" rows={rows:.0}"));
            }
            out.push(')');
        }
        out.push('\n');
        for (label, value) in [
            ("Join", &node.hash_condition),
            ("Index", &node.index_condition),
            ("Filter", &node.filter),
        ] {
            if let Some(value) = value {
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        for child in &node.children {
            write(child, depth + 1, out);
        }
    }
    let mut out = String::new();
    write(node, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operator(id: i64, operator_type: &str, total_cost: f64) -> Operator {
        Operator {
            id,
            operator_type: operator_type.to_string(),
            total_cost: Some(total_cost),
            first_row_cost: Some(total_cost / 2.0),
            ..Default::default()
        }
    }

    fn from_operator(source: i64, target: i64, count: f64) -> Stream {
        Stream {
            source: StreamSource::Operator(source),
            target,
            count: Some(count),
        }
    }

    fn from_object(schema: &str, name: &str, target: i64) -> Stream {
        Stream {
            source: StreamSource::Object {
                schema: schema.to_string(),
                name: name.to_string(),
            },
            target,
            count: Some(6.0),
        }
    }

    fn predicate(operator_id: i64, how_applied: &str, text: &str) -> Predicate {
        Predicate {
            operator_id,
            how_applied: how_applied.to_string(),
            text: text.to_string(),
        }
    }

    fn argument(operator_id: i64, kind: &str, value: &str) -> Argument {
        Argument {
            operator_id,
            kind: kind.to_string(),
            value: value.to_string(),
        }
    }

    /// The plan Db2 12.1 writes for
    /// `SELECT E.LAST_NAME, D.DEPT_NAME FROM EMPLOYEES E LEFT JOIN
    /// DEPARTMENTS D ON D.DEPT_ID = E.DEPT_ID WHERE E.EMP_ID BETWEEN 2 AND 5
    /// AND E.SALARY > 1000 ORDER BY E.LAST_NAME`.
    fn join_plan() -> PlanRows {
        let mut index_tables = HashMap::new();
        index_tables.insert(
            ("SYSIBM".to_string(), "SQL0001".to_string()),
            ("TEST_SCHEMA".to_string(), "EMPLOYEES".to_string()),
        );
        PlanRows {
            operators: vec![
                operator(1, "RETURN", 13.54),
                operator(2, "TBSCAN", 13.54),
                operator(3, "SORT", 13.54),
                operator(4, "HSJOIN", 13.54),
                operator(5, "TBSCAN", 6.77),
                operator(6, "FETCH", 6.77),
                operator(7, "IXSCAN", 0.0045),
            ],
            streams: vec![
                from_operator(2, 1, 3.78),
                from_operator(3, 2, 3.78),
                from_operator(4, 3, 3.78),
                // Listed inner first, to show the outer input still leads.
                from_operator(6, 4, 3.78),
                from_operator(5, 4, 4.0),
                from_object("TEST_SCHEMA", "DEPARTMENTS", 5),
                from_object("TEST_SCHEMA", "EMPLOYEES", 6),
                from_operator(7, 6, 4.0),
                from_object("SYSIBM", "SQL0001", 7),
            ],
            predicates: vec![
                predicate(4, "JOIN", "(Q1.DEPT_ID = Q3.DEPT_ID)"),
                predicate(6, "SARG", "(1000 < Q2.SALARY)"),
                predicate(7, "STOP", "(Q2.EMP_ID <= 5)"),
                predicate(7, "START", "(2 <= Q2.EMP_ID)"),
            ],
            arguments: vec![
                argument(1, "PREPTIME", "        46 milliseconds"),
                argument(3, "SORTKEY", "1: Q4.LAST_NAME(A)"),
                argument(4, "OUTERJN", "RIGHT"),
                argument(5, "JN INPUT", "OUTER"),
                argument(6, "JN INPUT", "INNER"),
            ],
            index_tables,
        }
    }

    fn summary(node: &ExplainNode, depth: usize, out: &mut Vec<String>) {
        out.push(format!(
            "{}{} {}",
            "  ".repeat(depth),
            node.node_type,
            node.relation.as_deref().unwrap_or("-")
        ));
        for child in &node.children {
            summary(child, depth + 1, out);
        }
    }

    fn find<'a>(node: &'a ExplainNode, id: &str) -> &'a ExplainNode {
        fn search<'a>(node: &'a ExplainNode, id: &str) -> Option<&'a ExplainNode> {
            if node.id == id {
                return Some(node);
            }
            node.children.iter().find_map(|child| search(child, id))
        }
        search(node, id).unwrap_or_else(|| panic!("no operator {id}"))
    }

    #[test]
    fn builds_the_tree_with_tables_in_place_of_data_objects() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        let mut lines = Vec::new();
        summary(&root, 0, &mut lines);
        assert_eq!(
            lines,
            [
                "RETURN -",
                "  TBSCAN -",
                "    SORT -",
                "      HSJOIN -",
                "        TBSCAN DEPARTMENTS",
                "        FETCH EMPLOYEES",
                "          IXSCAN EMPLOYEES",
            ]
        );
    }

    #[test]
    fn reads_estimates_and_costs() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        // The top operator passes its rows to the application.
        assert_eq!(root.plan_rows, Some(3.78));
        assert_eq!(root.total_cost, Some(13.54));
        assert_eq!(root.startup_cost, Some(6.77));
        assert_eq!(find(&root, "5").plan_rows, Some(4.0));
    }

    #[test]
    fn places_predicates_where_they_apply() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        let join = find(&root, "4");
        assert_eq!(
            join.hash_condition.as_deref(),
            Some("(Q1.DEPT_ID = Q3.DEPT_ID)")
        );
        assert_eq!(join.join_type.as_deref(), Some("Right"));
        assert_eq!(
            find(&root, "6").filter.as_deref(),
            Some("(1000 < Q2.SALARY)")
        );
        let index_scan = find(&root, "7");
        assert_eq!(
            index_scan.index_condition.as_deref(),
            Some("(Q2.EMP_ID <= 5) AND (2 <= Q2.EMP_ID)")
        );
        assert_eq!(index_scan.extra["index"], "SYSIBM.SQL0001");
    }

    #[test]
    fn marks_only_table_scans_as_full_scans() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        assert_eq!(find(&root, "5").extra["access_type"], "ALL");
        // The scan of the sorted rows reads a temporary table, not a table.
        assert!(!find(&root, "2").extra.contains_key("access_type"));
        assert!(!find(&root, "7").extra.contains_key("access_type"));
    }

    #[test]
    fn keeps_operator_arguments() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        assert_eq!(find(&root, "3").extra["SORTKEY"], "1: Q4.LAST_NAME(A)");
        assert_eq!(planning_time_ms(&join_plan()), Some(46.0));
    }

    #[test]
    fn qualifies_objects_of_other_schemas() {
        let root = build_plan_tree(&join_plan(), "OTHER").expect("a root");
        assert_eq!(
            find(&root, "5").relation.as_deref(),
            Some("TEST_SCHEMA.DEPARTMENTS")
        );
    }

    #[test]
    fn equal_start_and_stop_predicates_are_listed_once() {
        let mut rows = join_plan();
        rows.predicates = vec![
            predicate(7, "START", "(Q2.EMP_ID = 3)"),
            predicate(7, "STOP", "(Q2.EMP_ID = 3)"),
        ];
        let root = build_plan_tree(&rows, "TEST_SCHEMA").expect("a root");
        assert_eq!(
            find(&root, "7").index_condition.as_deref(),
            Some("(Q2.EMP_ID = 3)")
        );
    }

    #[test]
    fn inner_joins_and_other_joins() {
        let mut rows = join_plan();
        rows.arguments.retain(|argument| argument.kind != "OUTERJN");
        rows.operators[3].operator_type = "NLJOIN".to_string();
        let root = build_plan_tree(&rows, "TEST_SCHEMA").expect("a root");
        let join = find(&root, "4");
        assert_eq!(join.join_type.as_deref(), Some("Inner"));
        // Only a hash join has a hash condition.
        assert_eq!(join.hash_condition, None);
        assert_eq!(join.extra["join_condition"], "(Q1.DEPT_ID = Q3.DEPT_ID)");
        assert_eq!(find(&root, "5").join_type, None);
    }

    #[test]
    fn a_shared_input_is_drawn_under_each_reader() {
        let rows = PlanRows {
            operators: vec![
                operator(1, "RETURN", 3.0),
                operator(2, "UNION", 3.0),
                operator(3, "TBSCAN", 1.0),
                operator(4, "TBSCAN", 1.0),
                operator(5, "TEMP", 1.0),
            ],
            streams: vec![
                from_operator(2, 1, 2.0),
                from_operator(3, 2, 1.0),
                from_operator(4, 2, 1.0),
                from_operator(5, 3, 1.0),
                from_operator(5, 4, 1.0),
            ],
            ..Default::default()
        };
        let root = build_plan_tree(&rows, "S").expect("a root");
        let mut lines = Vec::new();
        summary(&root, 0, &mut lines);
        assert_eq!(
            lines,
            [
                "RETURN -",
                "  UNION -",
                "    TBSCAN -",
                "      TEMP -",
                "    TBSCAN -",
                "      TEMP -",
            ]
        );
    }

    #[test]
    fn a_loop_does_not_recurse_forever() {
        let rows = PlanRows {
            operators: vec![operator(1, "RETURN", 1.0), operator(2, "TBSCAN", 1.0)],
            streams: vec![from_operator(2, 1, 1.0), from_operator(1, 2, 1.0)],
            ..Default::default()
        };
        // Every operator has a consumer: there is no root to start from.
        assert!(build_plan_tree(&rows, "S").is_none());
    }

    #[test]
    fn no_operators_no_tree() {
        assert!(build_plan_tree(&PlanRows::default(), "S").is_none());
    }

    #[test]
    fn plan_text_shows_relations_and_conditions() {
        let root = build_plan_tree(&join_plan(), "TEST_SCHEMA").expect("a root");
        let text = plan_text(&root);
        assert!(text.starts_with("1 RETURN (cost=13.54 rows=4)\n"), "{text}");
        assert!(
            text.contains(
                "      4 HSJOIN (cost=13.54 rows=4)\n        Join: (Q1.DEPT_ID = Q3.DEPT_ID)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("          7 IXSCAN on EMPLOYEES using SYSIBM.SQL0001"),
            "{text}"
        );
    }

    #[test]
    fn preparation_time_units() {
        let with = |value: &str| PlanRows {
            arguments: vec![argument(1, "PREPTIME", value)],
            ..Default::default()
        };
        assert_eq!(planning_time_ms(&with("2 seconds")), Some(2000.0));
        assert_eq!(planning_time_ms(&with("500 microseconds")), Some(0.5));
        assert_eq!(planning_time_ms(&with("soon")), None);
        assert_eq!(planning_time_ms(&PlanRows::default()), None);
    }
}
