//! `explain_query`: Db2's access plan for a statement, for Tabularis's
//! Visual EXPLAIN.
//!
//! `EXPLAIN PLAN` compiles the statement without running it and writes the
//! plan into the explain tables. Each request tags its statement with a
//! `QUERYTAG` of its own, reads back only the rows with that tag and
//! deletes them afterwards, so concurrent explains never mix and the tables
//! do not grow with every click.
//!
//! The rows go back as they are, in the raw form Tabularis hands to the
//! parser this plugin ships (`explain/`), which builds the tree.

use crate::client::Session;
use crate::error::PluginResult;
use crate::handlers::query::single_statement;
use crate::models::{
    Db2ExplainPayload, ExplainAnalyze, ExplainArgument, ExplainIndex, ExplainOperator,
    ExplainPredicate, ExplainStream, RawExplain,
};
use crate::utils::identifiers::quote_identifier;
use crate::utils::values::string_literal;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// What Tabularis matches the parser on; `.tabularium` declares the same
/// pair under `explain_parsers`.
pub const ENGINE: &str = "db2";
pub const FORMAT: &str = "db2-explain-tables-json";

fn cell(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index).and_then(|value| value.as_deref())
}

fn text(row: &[Option<String>], index: usize) -> String {
    cell(row, index).unwrap_or_default().trim().to_string()
}

fn number(row: &[Option<String>], index: usize) -> Option<f64> {
    cell(row, index).and_then(|value| value.trim().parse::<f64>().ok())
}

fn integer(row: &[Option<String>], index: usize) -> i64 {
    cell(row, index)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or_default()
}

/// The schema of the explain tables `EXPLAIN` writes to: the session user's
/// own when there are some, else `SYSTOOLS`. When neither has them they
/// are created in `SYSTOOLS`, which takes the authority to create tables
/// there.
fn explain_tables(session: &Session) -> PluginResult<String> {
    let found = session.query(
        "SELECT TABSCHEMA FROM SYSCAT.TABLES \
         WHERE TABNAME = 'EXPLAIN_INSTANCE' AND TABSCHEMA IN (SESSION_USER, 'SYSTOOLS') \
         ORDER BY CASE WHEN TABSCHEMA = 'SYSTOOLS' THEN 1 ELSE 0 END \
         FETCH FIRST 1 ROW ONLY WITH UR",
    )?;
    if let Some(schema) = found.rows.first().map(|row| text(row, 0)) {
        return Ok(schema);
    }
    session
        .execute(
            "CALL SYSPROC.SYSINSTALLOBJECTS('EXPLAIN', 'C', \
             CAST(NULL AS VARCHAR(128)), CAST(NULL AS VARCHAR(128)))",
        )
        .map_err(|error| {
            format!(
                "EXPLAIN needs the Db2 explain tables, and creating them in SYSTOOLS failed: {error}"
            )
        })?;
    Ok("SYSTOOLS".to_string())
}

/// A `QUERYTAG` (at most 20 characters) no other request uses.
fn query_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let unique = (nanos as u64) ^ (u64::from(std::process::id()) << 40);
    format!("TABULARIS{:011X}", unique & 0xFFF_FFFF_FFFF)
}

/// `FROM` one explain table, joined to the statement each row belongs to
/// as `S`; the table itself is `X`.
fn plan_table(tables: &str, table: &str) -> String {
    format!(
        "FROM {tables}.{table} X JOIN {tables}.EXPLAIN_STATEMENT S \
         ON S.EXPLAIN_REQUESTER = X.EXPLAIN_REQUESTER AND S.EXPLAIN_TIME = X.EXPLAIN_TIME \
         AND S.SOURCE_NAME = X.SOURCE_NAME AND S.SOURCE_SCHEMA = X.SOURCE_SCHEMA \
         AND S.SOURCE_VERSION = X.SOURCE_VERSION AND S.EXPLAIN_LEVEL = X.EXPLAIN_LEVEL \
         AND S.STMTNO = X.STMTNO AND S.SECTNO = X.SECTNO"
    )
}

/// `WHERE` the statement is the tagged one, at the level of its access plan
/// (Db2 also records the original statement, level `O`, without operators).
fn tagged(tag: &str) -> String {
    format!(
        "WHERE S.QUERYTAG = {} AND S.EXPLAIN_LEVEL = 'P'",
        string_literal(tag)
    )
}

/// The rows of one explain table that belong to the tagged plan.
fn plan_rows_of(tables: &str, table: &str, tag: &str) -> String {
    format!("{} {}", plan_table(tables, table), tagged(tag))
}

fn optional_text(row: &[Option<String>], index: usize) -> Option<String> {
    cell(row, index)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_plan(
    session: &Session,
    tables: &str,
    tag: &str,
    schema: &str,
) -> PluginResult<Db2ExplainPayload> {
    let operators = session
        .query(&format!(
            "SELECT X.OPERATOR_ID, X.OPERATOR_TYPE, X.TOTAL_COST, X.IO_COST, X.CPU_COST, \
             X.FIRST_ROW_COST, X.RE_TOTAL_COST, X.BUFFERS {} ORDER BY X.OPERATOR_ID",
            plan_rows_of(tables, "EXPLAIN_OPERATOR", tag)
        ))?
        .rows
        .iter()
        .map(|row| ExplainOperator {
            id: integer(row, 0),
            operator_type: text(row, 1),
            total_cost: number(row, 2),
            io_cost: number(row, 3),
            cpu_cost: number(row, 4),
            first_row_cost: number(row, 5),
            re_total_cost: number(row, 6),
            buffers: number(row, 7),
        })
        .collect();

    let streams = session
        .query(&format!(
            "SELECT X.SOURCE_TYPE, X.SOURCE_ID, X.TARGET_ID, X.OBJECT_SCHEMA, X.OBJECT_NAME, \
             X.STREAM_COUNT {} AND X.TARGET_TYPE = 'O' ORDER BY X.TARGET_ID, X.SOURCE_ID",
            plan_rows_of(tables, "EXPLAIN_STREAM", tag)
        ))?
        .rows
        .iter()
        .map(|row| ExplainStream {
            source_type: text(row, 0),
            source_id: integer(row, 1),
            target_id: integer(row, 2),
            object_schema: optional_text(row, 3),
            object_name: optional_text(row, 4),
            count: number(row, 5),
        })
        .collect();

    let predicates = session
        .query(&format!(
            "SELECT X.OPERATOR_ID, X.HOW_APPLIED, X.PREDICATE_TEXT {} \
             ORDER BY X.OPERATOR_ID, X.PREDICATE_ID",
            plan_rows_of(tables, "EXPLAIN_PREDICATE", tag)
        ))?
        .rows
        .iter()
        .filter(|row| cell(row, 2).is_some())
        .map(|row| ExplainPredicate {
            operator_id: integer(row, 0),
            how_applied: text(row, 1),
            text: text(row, 2),
        })
        .collect();

    let arguments = session
        .query(&format!(
            "SELECT X.OPERATOR_ID, X.ARGUMENT_TYPE, \
             COALESCE(X.ARGUMENT_VALUE, X.LONG_ARGUMENT_VALUE) {} \
             ORDER BY X.OPERATOR_ID, X.ARGUMENT_TYPE",
            plan_rows_of(tables, "EXPLAIN_ARGUMENT", tag)
        ))?
        .rows
        .iter()
        .map(|row| ExplainArgument {
            operator_id: integer(row, 0),
            argument_type: text(row, 1),
            value: text(row, 2),
        })
        .collect();

    // The plan names the indexes it reads; their tables are in the catalog.
    let indexes = session
        .query(&format!(
            "SELECT X.OBJECT_SCHEMA, X.OBJECT_NAME, I.TABSCHEMA, I.TABNAME {} \
             JOIN SYSCAT.INDEXES I \
             ON I.INDSCHEMA = X.OBJECT_SCHEMA AND I.INDNAME = X.OBJECT_NAME \
             {} AND X.OBJECT_TYPE = 'IX' ORDER BY 1, 2 WITH UR",
            plan_table(tables, "EXPLAIN_OBJECT"),
            tagged(tag)
        ))?
        .rows
        .iter()
        .map(|row| ExplainIndex {
            schema: text(row, 0),
            name: text(row, 1),
            table_schema: text(row, 2),
            table_name: text(row, 3),
        })
        .collect();

    Ok(Db2ExplainPayload {
        db2_explain: 1,
        schema: schema.to_string(),
        operators,
        streams,
        predicates,
        arguments,
        indexes,
        analyze: None,
    })
}

/// Deletes the tagged explain rows. The explain tables cascade from
/// `EXPLAIN_INSTANCE`. A failure only leaves rows behind, so it is not
/// reported.
fn discard_plan(session: &Session, tables: &str, tag: &str) {
    let _ = session.execute(&format!(
        "DELETE FROM {tables}.EXPLAIN_INSTANCE I WHERE EXISTS (\
         SELECT 1 FROM {tables}.EXPLAIN_STATEMENT S WHERE S.QUERYTAG = {} \
         AND S.EXPLAIN_REQUESTER = I.EXPLAIN_REQUESTER AND S.EXPLAIN_TIME = I.EXPLAIN_TIME \
         AND S.SOURCE_NAME = I.SOURCE_NAME AND S.SOURCE_SCHEMA = I.SOURCE_SCHEMA \
         AND S.SOURCE_VERSION = I.SOURCE_VERSION)",
        string_literal(tag)
    ));
}

/// The access plan of `query`, read in the schema the session works in.
///
/// With `analyze` the statement is also run, as Tabularis's Analyze option
/// promises, and the payload carries what Db2 can say about that run without
/// an activity event monitor: the time it took and the number of rows it
/// returned or changed.
pub fn explain_query(
    session: &Session,
    schema: &str,
    query: &str,
    analyze: bool,
) -> PluginResult<RawExplain> {
    let statement = single_statement(query)?;
    let tables = quote_identifier(&explain_tables(session)?);
    let tag = query_tag();

    if let Err(error) = session.execute(&format!(
        "EXPLAIN PLAN SET QUERYTAG = {} FOR {statement}",
        string_literal(&tag)
    )) {
        // An error in the statement is reported by Db2 as an error in the
        // EXPLAIN, quoting text the user never wrote: compile the statement
        // alone to report its own error instead.
        session.check(statement)?;
        return Err(if error.contains("SQL0104N") {
            "Db2 has no access plan for this statement: only queries and data \
             changes (SELECT, VALUES, INSERT, UPDATE, DELETE, MERGE) can be explained"
                .to_string()
        } else {
            error
        });
    }
    let payload = read_plan(session, &tables, &tag, schema);
    discard_plan(session, &tables, &tag);
    let mut payload = payload?;
    if payload.operators.is_empty() {
        return Err("Db2 wrote no access plan for this statement".to_string());
    }

    if analyze {
        let started = Instant::now();
        let rows = session.drain(statement)?;
        payload.analyze = Some(ExplainAnalyze {
            rows,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    }

    Ok(RawExplain {
        engine: ENGINE,
        format: FORMAT,
        payload: serde_json::to_string(&payload).map_err(|error| error.to_string())?,
        original_query: query.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{plan_rows_of, query_tag};
    use crate::models::Db2ExplainPayload;

    #[test]
    fn query_tags_fit_the_column() {
        let tag = query_tag();
        assert_eq!(tag.len(), 20, "{tag}");
        assert!(tag.starts_with("TABULARIS"));
    }

    #[test]
    fn plan_rows_are_those_of_the_tagged_access_plan() {
        let sql = plan_rows_of("\"SYSTOOLS\"", "EXPLAIN_OPERATOR", "T'1");
        assert!(sql.starts_with(
            "FROM \"SYSTOOLS\".EXPLAIN_OPERATOR X JOIN \"SYSTOOLS\".EXPLAIN_STATEMENT S"
        ));
        assert!(sql.ends_with("WHERE S.QUERYTAG = 'T''1' AND S.EXPLAIN_LEVEL = 'P'"));
    }

    #[test]
    fn the_version_key_comes_first() {
        // The parser recognises a Db2 payload by its first bytes.
        let payload = serde_json::to_string(&Db2ExplainPayload {
            db2_explain: 1,
            ..Default::default()
        })
        .expect("payload");
        assert!(payload.starts_with("{\"db2_explain\":1,"), "{payload}");
    }
}
