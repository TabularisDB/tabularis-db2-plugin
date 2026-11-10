use crate::client::{Outcome, Session, Window};
use crate::error::PluginResult;
use crate::models::{BatchStatementResult, Pagination, QueryResult};
use crate::utils::pagination::page_offset;
use crate::utils::values::cell_to_json;
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
