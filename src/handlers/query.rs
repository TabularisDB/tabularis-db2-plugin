use crate::client::Session;
use crate::error::PluginResult;
use crate::models::{ExplainNode, ExplainPlan, Pagination, QueryResult};
use crate::utils::pagination::page_offset;
use crate::utils::values::text_row_to_json;
use std::collections::HashMap;

pub fn test_connection(session: &Session) -> PluginResult<()> {
    session.query("SELECT CURRENT TIMESTAMP FROM SYSIBM.SYSDUMMY1 WITH UR")?;
    Ok(())
}

pub fn execute_query(
    session: &Session,
    query: &str,
    limit: Option<u32>,
    page: u32,
) -> PluginResult<QueryResult> {
    let trimmed = query.trim();
    let is_select_like = trimmed
        .chars()
        .take(10)
        .collect::<String>()
        .to_uppercase()
        .starts_with("SELECT")
        || trimmed.to_uppercase().starts_with("WITH");

    if !is_select_like {
        let affected_rows = session.execute(query)?;
        return Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows,
            truncated: false,
            pagination: None,
        });
    }

    let page_size = limit.unwrap_or(200);
    let paged_sql = format!(
        "SELECT * FROM ({}) AS TABULARIS_PAGE OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
        trimmed,
        page_offset(page, page_size),
        page_size
    );
    let result = session.query(&paged_sql)?;
    let rows = result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| text_row_to_json(value.as_deref()))
                .collect()
        })
        .collect::<Vec<Vec<serde_json::Value>>>();
    let has_more = rows.len() as u32 == page_size;

    Ok(QueryResult {
        columns: result.columns,
        rows,
        affected_rows: 0,
        truncated: false,
        pagination: Some(Pagination {
            page,
            page_size,
            total_rows: None,
            has_more,
        }),
    })
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
