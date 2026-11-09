use crate::client::{Outcome, Session, Window};
use crate::error::PluginResult;
use crate::models::{BatchStatementResult, ExplainNode, ExplainPlan, Pagination, QueryResult};
use crate::utils::pagination::page_offset;
use crate::utils::values::cell_to_json;
use std::collections::HashMap;
use std::time::Instant;

pub fn test_connection(session: &Session) -> PluginResult<()> {
    session.query("SELECT CURRENT TIMESTAMP FROM SYSIBM.SYSDUMMY1 WITH UR")?;
    Ok(())
}

/// Runs one statement and returns its rows or the number of rows it changed.
///
/// With a `limit` the result set is paged: page `page` (1-based) of `limit`
/// rows, with `has_more` telling whether another page exists. Without one,
/// every row is returned and there is no pagination block.
pub fn execute_query(
    session: &Session,
    query: &str,
    limit: Option<u32>,
    page: u32,
) -> PluginResult<QueryResult> {
    let statement = host_statement(single_statement(query)?);
    let page = page.max(1);
    let window = limit.map(|page_size| Window {
        offset: page_offset(page, page_size),
        limit: u64::from(page_size),
    });

    match session.run(statement, window)? {
        Outcome::Affected(affected_rows) => Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows,
            truncated: false,
            pagination: None,
        }),
        Outcome::Rows(result) => {
            let rows = result
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .zip(&result.kinds)
                        .map(|(value, kind)| cell_to_json(value.as_deref(), *kind))
                        .collect()
                })
                .collect::<Vec<Vec<serde_json::Value>>>();

            Ok(QueryResult {
                columns: result.columns,
                rows,
                affected_rows: 0,
                truncated: result.has_more,
                pagination: limit.map(|page_size| Pagination {
                    page,
                    page_size,
                    total_rows: None,
                    has_more: result.has_more,
                }),
            })
        }
    }
}

/// Runs the statements of a script one after the other on the same
/// connection, so what one statement sets up for the session (a declared
/// temporary table, `SET CURRENT SCHEMA`, a session variable) is still there
/// for the next.
///
/// A failing statement does not stop the script: its error is recorded and
/// the following statements still run, which is what Tabularis does itself
/// when a driver has no batch method. Every statement is committed on its
/// own (the connection is in autocommit mode).
pub fn execute_query_batch(
    session: &Session,
    queries: &[String],
    limit: Option<u32>,
    page: u32,
) -> Vec<BatchStatementResult> {
    queries
        .iter()
        .map(|query| {
            let started = Instant::now();
            let outcome = execute_query(session, query, limit, page);
            let execution_time_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            match outcome {
                Ok(result) => BatchStatementResult {
                    result: Some(result),
                    error: None,
                    execution_time_ms,
                },
                Err(error) => BatchStatementResult {
                    result: None,
                    error: Some(error),
                    execution_time_ms,
                },
            }
        })
        .collect()
}

/// Translates the one statement Tabularis writes itself, for every network
/// driver, in a form Db2 does not accept.
///
/// "Set to server NOW" in the data grid asks the driver for the server time
/// with a fixed `SELECT NOW()`. Db2 has no `FROM`-less `SELECT`, so the
/// action would only ever show a syntax error. `VALUES CURRENT TIMESTAMP`
/// returns the same thing. Only that exact text is rewritten; a query the
/// user wrote is always sent as written.
fn host_statement(statement: &str) -> &str {
    if statement.eq_ignore_ascii_case("SELECT NOW()") {
        "VALUES CURRENT TIMESTAMP"
    } else {
        statement
    }
}

/// Prepares the text of one statement for Db2: surrounding whitespace and
/// the statement terminators an editor leaves at the end are removed, since
/// CLI rejects a trailing `;`. A compound statement keeps the terminators
/// inside its body (`BEGIN ...; END`); only the ones after the last token
/// go.
pub fn single_statement(query: &str) -> PluginResult<&str> {
    let statement = query.trim().trim_end_matches([';', ' ', '\t', '\r', '\n']);
    if statement.is_empty() {
        return Err("The query is empty".to_string());
    }
    Ok(statement)
}

pub fn explain_query(session: &Session, query: &str, analyze: bool) -> PluginResult<ExplainPlan> {
    let prefix = if analyze {
        "EXPLAIN ALL FOR"
    } else {
        "EXPLAIN PLAN FOR"
    };
    let sql = format!("{prefix} {query}");

    // Try to run the EXPLAIN statement. If it fails because the explain tables
    // don't exist (SQL0219N / SQLSTATE 42704), create them automatically and retry.
    if let Err(err) = session.execute(&sql) {
        if err.contains("SQL0219N") || err.contains("42704") || err.contains("EXPLAIN_INSTANCE") {
            let _ = session.execute("CALL SYSPROC.SYSINSTALLOBJECTS('EXPLAIN', 'C', CAST(NULL AS VARCHAR(128)), CAST(NULL AS VARCHAR(128)))",
            )?;
            let _ = session.execute(&sql)?;
        } else {
            return Err(err);
        }
    }

    // Retrieve the explain_time of the most recent explain snapshot so we can
    // scope all subsequent queries to exactly this execution.
    let ts_result =
        session.query("SELECT VARCHAR(MAX(EXPLAIN_TIME)) AS TS FROM SYSTOOLS.EXPLAIN_INSTANCE")?;
    let explain_ts = ts_result
        .rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default();

    if explain_ts.is_empty() {
        return Ok(fallback_plan(query, analyze));
    }

    // ---- statement-level costs ----
    let stmt_result = session.query(&format!(
        "SELECT TOTAL_COST, STATEMENT_TEXT \
             FROM SYSTOOLS.EXPLAIN_STATEMENT \
             WHERE EXPLAIN_TIME = '{explain_ts}' \
             FETCH FIRST 1 ROWS ONLY"
    ))?;
    let stmt_cost = stmt_result
        .rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse::<f64>().ok());

    // ---- operators ----
    let ops_result = session.query(&format!(
        "SELECT OPERATOR_ID, OPERATOR_TYPE, TOTAL_COST, IO_COST, CPU_COST, \
                    FIRST_ROW_COST, RE_TOTAL_COST, BUFFERS \
             FROM SYSTOOLS.EXPLAIN_OPERATOR \
             WHERE EXPLAIN_TIME = '{explain_ts}' \
             ORDER BY OPERATOR_ID"
    ))?;

    // ---- streams (edges between operators) ----
    let streams_result = session.query(&format!(
        "SELECT SOURCE_ID, TARGET_ID, STREAM_COUNT, COLUMN_COUNT \
             FROM SYSTOOLS.EXPLAIN_STREAM \
             WHERE EXPLAIN_TIME = '{explain_ts}' \
             ORDER BY TARGET_ID, SOURCE_ID"
    ))?;

    // Build operator nodes keyed by ID.
    let mut nodes: HashMap<String, ExplainNode> = HashMap::new();
    for row in &ops_result.rows {
        let op_id = col_str(row, 0);
        let op_type = col_str(row, 1);
        let total_cost = col_f64(row, 2);
        let io_cost = col_f64(row, 3);
        let cpu_cost = col_f64(row, 4);
        let first_row_cost = col_f64(row, 5);
        let re_total_cost = col_f64(row, 6);
        let buffers = col_str_opt(row, 7);

        let mut extra = HashMap::new();
        if let Some(io) = io_cost {
            extra.insert("io_cost".to_string(), serde_json::json!(io));
        }
        if let Some(cpu) = cpu_cost {
            extra.insert("cpu_cost".to_string(), serde_json::json!(cpu));
        }
        if let Some(frc) = first_row_cost {
            extra.insert("first_row_cost".to_string(), serde_json::json!(frc));
        }
        if let Some(re) = re_total_cost {
            extra.insert("re_total_cost".to_string(), serde_json::json!(re));
        }
        if let Some(buf) = buffers {
            extra.insert("buffers".to_string(), serde_json::json!(buf));
        }

        nodes.insert(
            op_id.clone(),
            ExplainNode {
                id: op_id,
                node_type: op_type,
                relation: None,
                startup_cost: first_row_cost,
                total_cost,
                plan_rows: None,
                actual_rows: None,
                actual_time_ms: None,
                actual_loops: None,
                buffers_hit: None,
                buffers_read: None,
                filter: None,
                index_condition: None,
                join_type: None,
                hash_condition: None,
                extra,
                children: vec![],
            },
        );
    }

    // Wire parent-child relationships from streams.
    // A stream with SOURCE_ID -> TARGET_ID means the source feeds into the target,
    // so the source is a child of the target in the plan tree.
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut has_parent: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in &streams_result.rows {
        let source_id = col_str(row, 0);
        let target_id = col_str(row, 1);
        let stream_count = col_f64(row, 2);

        if !source_id.is_empty() && !target_id.is_empty() && source_id != target_id {
            // Annotate estimated rows on the source node from stream_count.
            if let Some(count) = stream_count {
                if let Some(node) = nodes.get_mut(&source_id) {
                    node.plan_rows = Some(count);
                }
            }
            children_map
                .entry(target_id)
                .or_default()
                .push(source_id.clone());
            has_parent.insert(source_id);
        }
    }

    // Assemble tree bottom-up: find the root (node with no parent).
    let root_id = nodes
        .keys()
        .find(|id| !has_parent.contains(id.as_str()))
        .cloned();

    let mut root = match root_id {
        Some(id) => build_tree(&id, &mut nodes, &children_map),
        None => fallback_plan(query, analyze).root,
    };

    // Use the statement-level total cost as a fallback for the root node.
    if root.total_cost.is_none() {
        root.total_cost = stmt_cost;
    }

    // Build a readable raw_output summary.
    let raw_output = format_raw_output(&root, 0);

    Ok(ExplainPlan {
        root,
        planning_time_ms: None,
        execution_time_ms: None,
        original_query: query.to_string(),
        driver: "db2".to_string(),
        has_analyze_data: analyze,
        raw_output: Some(raw_output),
    })
}

fn build_tree(
    id: &str,
    nodes: &mut HashMap<String, ExplainNode>,
    children_map: &HashMap<String, Vec<String>>,
) -> ExplainNode {
    let child_ids: Vec<String> = children_map.get(id).cloned().unwrap_or_default();
    let children: Vec<ExplainNode> = child_ids
        .iter()
        .map(|cid| build_tree(cid, nodes, children_map))
        .collect();
    let mut node = nodes.remove(id).unwrap_or_else(|| ExplainNode {
        id: id.to_string(),
        node_type: "Unknown".to_string(),
        relation: None,
        startup_cost: None,
        total_cost: None,
        plan_rows: None,
        actual_rows: None,
        actual_time_ms: None,
        actual_loops: None,
        buffers_hit: None,
        buffers_read: None,
        filter: None,
        index_condition: None,
        join_type: None,
        hash_condition: None,
        extra: Default::default(),
        children: vec![],
    });
    node.children = children;
    node
}

fn format_raw_output(node: &ExplainNode, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let cost_str = node
        .total_cost
        .map(|c| format!(" (cost={c:.2})"))
        .unwrap_or_default();
    let rows_str = node
        .plan_rows
        .map(|r| format!(" rows={r:.0}"))
        .unwrap_or_default();
    let mut line = format!(
        "{indent}{} [{}]{cost_str}{rows_str}\n",
        node.id, node.node_type
    );
    for child in &node.children {
        line.push_str(&format_raw_output(child, depth + 1));
    }
    line
}

fn fallback_plan(query: &str, analyze: bool) -> ExplainPlan {
    ExplainPlan {
        root: ExplainNode {
            id: "db2-explain".to_string(),
            node_type: "DB2 Explain".to_string(),
            relation: None,
            startup_cost: None,
            total_cost: None,
            plan_rows: None,
            actual_rows: None,
            actual_time_ms: None,
            actual_loops: None,
            buffers_hit: None,
            buffers_read: None,
            filter: None,
            index_condition: None,
            join_type: None,
            hash_condition: None,
            extra: Default::default(),
            children: vec![],
        },
        planning_time_ms: None,
        execution_time_ms: None,
        original_query: query.to_string(),
        driver: "db2".to_string(),
        has_analyze_data: analyze,
        raw_output: Some(
            "DB2 EXPLAIN executed but no plan data found in explain tables.".to_string(),
        ),
    }
}

fn col_str(row: &[Option<String>], idx: usize) -> String {
    row.get(idx)
        .and_then(|v| v.as_deref())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn col_str_opt(row: &[Option<String>], idx: usize) -> Option<String> {
    row.get(idx)
        .and_then(|v| v.as_deref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn col_f64(row: &[Option<String>], idx: usize) -> Option<f64> {
    row.get(idx)
        .and_then(|v| v.as_deref())
        .and_then(|s| s.trim().parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use super::{host_statement, single_statement};

    #[test]
    fn rewrites_only_the_hosts_server_time_probe() {
        assert_eq!(host_statement("SELECT NOW()"), "VALUES CURRENT TIMESTAMP");
        assert_eq!(host_statement("select now()"), "VALUES CURRENT TIMESTAMP");
        // Anything the user wrote goes to Db2 untouched, mistakes included.
        assert_eq!(host_statement("SELECT NOW() AS T"), "SELECT NOW() AS T");
        assert_eq!(
            host_statement("SELECT NOW() FROM SYSIBM.SYSDUMMY1"),
            "SELECT NOW() FROM SYSIBM.SYSDUMMY1"
        );
    }

    #[test]
    fn strips_trailing_terminators_only() {
        assert_eq!(single_statement("SELECT 1 FROM T;"), Ok("SELECT 1 FROM T"));
        assert_eq!(
            single_statement("  SELECT 1 FROM T ;\n;  "),
            Ok("SELECT 1 FROM T")
        );
        // The terminators inside a compound statement are part of it.
        assert_eq!(
            single_statement("BEGIN DECLARE X INT; SET X = 1; END;"),
            Ok("BEGIN DECLARE X INT; SET X = 1; END")
        );
        // A semicolon inside a string is data.
        assert_eq!(
            single_statement("SELECT ';' FROM T"),
            Ok("SELECT ';' FROM T")
        );
    }

    #[test]
    fn rejects_an_empty_statement() {
        assert!(single_statement("").is_err());
        assert!(single_statement("  ;\n ").is_err());
    }
}
