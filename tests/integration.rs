//! Integration tests for tabularis-db2-plugin.
//!
//! These tests spawn the plugin binary and communicate over JSON-RPC via stdio,
//! exactly as Tabularis does at runtime. They require a running DB2 instance
//! seeded with the test fixtures (see `scripts/setup-test-db.sh`).
//!
//! Run with:
//!   DB2_TEST=1 cargo test --test integration -- --test-threads=1

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

const TABLE_COMMENT: &str = "Employee's résumé\n所有员工";
const COLUMN_COMMENT: &str = "Manager's notes — café\n第二行";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

macro_rules! require_db2 {
    () => {
        if std::env::var("DB2_TEST").is_err() {
            eprintln!("Skipping: set DB2_TEST=1 to run integration tests");
            return;
        }
    };
}

fn conn_params() -> Value {
    json!({
        "driver": "db2",
        "host": std::env::var("DB2_HOST").unwrap_or_else(|_| "localhost".into()),
        "port": std::env::var("DB2_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(50000),
        "database": std::env::var("DB2_DATABASE").unwrap_or_else(|_| "TESTDB".into()),
        "username": std::env::var("DB2_USER").unwrap_or_else(|_| "db2inst1".into()),
        "password": std::env::var("DB2_PASSWORD").unwrap_or_else(|_| "db2test123".into()),
    })
}

fn driver_name() -> String {
    std::env::var("DB2_ODBC_DRIVER").unwrap_or_else(|_| "Db2".into())
}

struct Plugin {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    stdin: std::process::ChildStdin,
    next_id: u64,
}

impl Plugin {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tabularis-db2-plugin"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn plugin binary");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);

        let mut plugin = Self {
            child,
            reader,
            stdin,
            next_id: 1,
        };
        plugin.initialize();
        plugin
    }

    fn initialize(&mut self) {
        let resp = self.call(
            "initialize",
            json!({
                "settings": {
                    "driver_name": driver_name(),
                }
            }),
        );
        assert!(
            resp.get("result").is_some(),
            "initialize failed: {resp}"
        );
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();

        let mut buf = String::new();
        self.reader.read_line(&mut buf).expect("failed to read response");
        serde_json::from_str(&buf).expect("invalid JSON response")
    }

    fn call_ok(&mut self, method: &str, params: Value) -> Value {
        let resp = self.call(method, params);
        if let Some(err) = resp.get("error") {
            panic!("{method} returned error: {err}");
        }
        resp["result"].clone()
    }

    fn base_params(&self) -> Value {
        json!({ "params": conn_params() })
    }

    fn schema_params(&self, extras: Value) -> Value {
        let mut base = json!({
            "params": conn_params(),
            "schema": "TEST_SCHEMA",
        });
        if let Value::Object(map) = extras {
            for (k, v) in map {
                base[k] = v;
            }
        }
        base
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Tests — Connection
// ---------------------------------------------------------------------------

#[test]
fn test_connection_and_ping() {
    require_db2!();
    let mut p = Plugin::spawn();

    let resp = p.call("test_connection", p.base_params());
    assert!(resp.get("result").is_some(), "test_connection failed: {resp}");

    let resp = p.call("ping", p.base_params());
    assert!(resp.get("result").is_some(), "ping failed: {resp}");
}

// ---------------------------------------------------------------------------
// Tests — Database & Schema metadata
// ---------------------------------------------------------------------------

#[test]
fn test_get_databases() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_databases", p.base_params());
    let dbs = result.as_array().expect("expected array");
    assert!(!dbs.is_empty(), "expected at least one database");
}

#[test]
fn test_get_schemas() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_schemas", p.base_params());
    let schemas = result.as_array().expect("expected array");
    let names: Vec<&str> = schemas.iter().filter_map(Value::as_str).collect();
    assert!(
        names.iter().any(|s| s.trim() == "TEST_SCHEMA"),
        "TEST_SCHEMA not found in: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests — Table metadata
// ---------------------------------------------------------------------------

#[test]
fn test_get_tables() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_tables", p.schema_params(json!({})));
    let tables = result.as_array().expect("expected array");
    let names: Vec<&str> = tables
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    for expected in &["DEPARTMENTS", "EMPLOYEES", "PROJECTS", "EMP_PROJECTS", "DATA_TYPES_TEST"] {
        assert!(
            names.contains(expected),
            "table {expected} not found in: {names:?}"
        );
    }

    let employees = tables
        .iter()
        .find(|table| table["name"] == "EMPLOYEES")
        .expect("EMPLOYEES table missing");
    assert_eq!(employees["comment"], json!(TABLE_COMMENT));

    let departments = tables
        .iter()
        .find(|table| table["name"] == "DEPARTMENTS")
        .expect("DEPARTMENTS table missing");
    assert!(
        departments.get("comment").is_none(),
        "tables without remarks should omit comment"
    );
}

#[test]
fn test_get_columns() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": "EMPLOYEES" })),
    );
    let columns = result.as_array().expect("expected array");

    // Verify we got all expected columns
    let col_names: Vec<&str> = columns
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for expected in &["EMP_ID", "FIRST_NAME", "LAST_NAME", "EMAIL", "SALARY", "DEPT_ID"] {
        assert!(
            col_names.contains(expected),
            "column {expected} not found in: {col_names:?}"
        );
    }

    // Check PK flag on EMP_ID
    let emp_id = columns
        .iter()
        .find(|c| c["name"].as_str() == Some("EMP_ID"))
        .expect("EMP_ID column missing");
    assert_eq!(emp_id["is_pk"], json!(true), "EMP_ID should be PK");
    assert_eq!(
        emp_id["is_auto_increment"],
        json!(true),
        "EMP_ID should be auto-increment"
    );

    // Check nullable
    let dept_id = columns
        .iter()
        .find(|c| c["name"].as_str() == Some("DEPT_ID"))
        .expect("DEPT_ID column missing");
    assert_eq!(
        dept_id["is_nullable"],
        json!(true),
        "DEPT_ID should be nullable"
    );

    let notes = columns
        .iter()
        .find(|column| column["name"] == "NOTES")
        .expect("NOTES column missing");
    assert_eq!(notes["comment"], json!(COLUMN_COMMENT));

    let first_name = columns
        .iter()
        .find(|column| column["name"] == "FIRST_NAME")
        .expect("FIRST_NAME column missing");
    assert!(
        first_name.get("comment").is_none(),
        "columns without remarks should omit comment"
    );
}

#[test]
fn test_get_all_columns_batch() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_all_columns_batch", p.schema_params(json!({})));
    let map = result.as_object().expect("expected object");

    assert!(
        map.contains_key("EMPLOYEES"),
        "batch should include EMPLOYEES"
    );
    assert!(
        map.contains_key("DEPARTMENTS"),
        "batch should include DEPARTMENTS"
    );

    let emp_cols = map["EMPLOYEES"].as_array().expect("expected array");
    assert!(
        emp_cols.len() >= 6,
        "EMPLOYEES should have at least 6 columns"
    );

    let notes = emp_cols
        .iter()
        .find(|column| column["name"] == "NOTES")
        .expect("NOTES column missing from batch");
    assert_eq!(notes["comment"], json!(COLUMN_COMMENT));

    let first_name = emp_cols
        .iter()
        .find(|column| column["name"] == "FIRST_NAME")
        .expect("FIRST_NAME column missing from batch");
    assert!(
        first_name.get("comment").is_none(),
        "batch columns without remarks should omit comment"
    );
}

// ---------------------------------------------------------------------------
// Tests — View metadata
// ---------------------------------------------------------------------------

#[test]
fn test_get_views() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_views", p.schema_params(json!({})));
    let views = result.as_array().expect("expected array");
    let names: Vec<&str> = views
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();

    assert!(
        names.contains(&"V_EMP_DETAILS"),
        "V_EMP_DETAILS not found in: {names:?}"
    );
    assert!(
        names.contains(&"V_DEPT_SUMMARY"),
        "V_DEPT_SUMMARY not found in: {names:?}"
    );
}

#[test]
fn test_get_view_definition() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_view_definition",
        p.schema_params(json!({ "view_name": "V_EMP_DETAILS" })),
    );
    let definition = result.as_str().expect("expected string");
    // DB2 stores view definitions in upper case or as-is
    let upper = definition.to_uppercase();
    assert!(
        upper.contains("EMPLOYEES") && upper.contains("DEPARTMENTS"),
        "view definition should reference EMPLOYEES and DEPARTMENTS: {definition}"
    );
}

#[test]
fn test_get_view_columns() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_view_columns",
        p.schema_params(json!({ "view_name": "V_EMP_DETAILS" })),
    );
    let columns = result.as_array().expect("expected array");
    let names: Vec<&str> = columns
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();

    assert!(
        names.contains(&"FIRST_NAME"),
        "view should expose FIRST_NAME: {names:?}"
    );
    assert!(
        names.contains(&"DEPT_NAME"),
        "view should expose DEPT_NAME: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests — Foreign keys & Indexes
// ---------------------------------------------------------------------------

#[test]
fn test_get_foreign_keys() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_foreign_keys",
        p.schema_params(json!({ "table": "EMPLOYEES" })),
    );
    let fks = result.as_array().expect("expected array");

    let fk = fks
        .iter()
        .find(|f| f["name"].as_str().map(|n| n.contains("FK_EMP_DEPT")).unwrap_or(false))
        .expect("FK_EMP_DEPT not found");

    assert_eq!(fk["column_name"].as_str().unwrap().trim(), "DEPT_ID");
    assert_eq!(fk["ref_table"].as_str().unwrap().trim(), "DEPARTMENTS");
}

#[test]
fn test_get_all_foreign_keys_batch() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_all_foreign_keys_batch", p.schema_params(json!({})));
    let map = result.as_object().expect("expected object");

    assert!(
        map.contains_key("EMPLOYEES"),
        "batch should include EMPLOYEES FK"
    );
    assert!(
        map.contains_key("EMP_PROJECTS"),
        "batch should include EMP_PROJECTS FKs"
    );
}

#[test]
fn test_get_indexes() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_indexes",
        p.schema_params(json!({ "table": "EMPLOYEES" })),
    );
    let indexes = result.as_array().expect("expected array");
    let names: Vec<&str> = indexes
        .iter()
        .filter_map(|i| i["name"].as_str())
        .collect();

    assert!(
        names.iter().any(|n| n.contains("IDX_EMP_EMAIL")),
        "IDX_EMP_EMAIL not found in: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("IDX_EMP_LAST_NAME")),
        "IDX_EMP_LAST_NAME not found in: {names:?}"
    );

    // Check unique flag on email index
    let email_idx = indexes
        .iter()
        .find(|i| {
            i["name"]
                .as_str()
                .map(|n| n.contains("IDX_EMP_EMAIL"))
                .unwrap_or(false)
        })
        .expect("email index missing");
    assert_eq!(
        email_idx["is_unique"],
        json!(true),
        "email index should be unique"
    );
}

// ---------------------------------------------------------------------------
// Tests — Routines
// ---------------------------------------------------------------------------

#[test]
fn test_get_routines() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_routines", p.schema_params(json!({})));
    let routines = result.as_array().expect("expected array");
    let names: Vec<&str> = routines
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();

    assert!(
        names.contains(&"GET_EMPLOYEES_BY_DEPT"),
        "procedure GET_EMPLOYEES_BY_DEPT missing: {names:?}"
    );
    assert!(
        names.contains(&"FULL_NAME"),
        "function FULL_NAME missing: {names:?}"
    );
}

#[test]
fn test_get_routine_parameters() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_routine_parameters",
        p.schema_params(json!({ "routine_name": "UPDATE_SALARY" })),
    );
    let params = result.as_array().expect("expected array");
    assert!(params.len() >= 3, "UPDATE_SALARY should have >= 3 params");

    let modes: Vec<&str> = params
        .iter()
        .filter_map(|p| p["mode"].as_str())
        .collect();
    assert!(modes.contains(&"IN"), "should have IN params");
    assert!(modes.contains(&"OUT"), "should have OUT param");
}

#[test]
fn test_get_routine_definition() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_routine_definition",
        p.schema_params(json!({ "routine_name": "FULL_NAME" })),
    );
    let def = result.as_str().expect("expected string");
    let upper = def.to_uppercase();
    assert!(
        upper.contains("RETURN"),
        "function definition should contain RETURN: {def}"
    );
}

// ---------------------------------------------------------------------------
// Tests — Schema snapshot
// ---------------------------------------------------------------------------

#[test]
fn test_get_schema_snapshot() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_schema_snapshot", p.schema_params(json!({})));
    let tables = result.as_array().expect("expected array");

    let names: Vec<&str> = tables
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"EMPLOYEES"),
        "snapshot should include EMPLOYEES"
    );

    // Check that snapshot includes columns
    let emp = tables
        .iter()
        .find(|t| t["name"].as_str() == Some("EMPLOYEES"))
        .unwrap();
    let cols = emp["columns"].as_array().expect("expected columns array");
    assert!(!cols.is_empty(), "snapshot should include column data");
    let notes = cols
        .iter()
        .find(|column| column["name"] == "NOTES")
        .expect("snapshot should include NOTES");
    assert_eq!(notes["comment"], json!(COLUMN_COMMENT));

    // Check that snapshot includes foreign keys
    let fks = emp["foreign_keys"]
        .as_array()
        .expect("expected foreign_keys array");
    assert!(!fks.is_empty(), "EMPLOYEES snapshot should include FK data");
}

// ---------------------------------------------------------------------------
// Tests — Query execution
// ---------------------------------------------------------------------------

#[test]
fn test_execute_query() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT EMP_ID, FIRST_NAME, LAST_NAME, SALARY FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID",
            "limit": 10,
            "page": 1,
        })),
    );

    let columns = result["columns"].as_array().expect("columns array");
    assert_eq!(columns.len(), 4);

    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 6, "should return all 6 employees");

    // First row should be EMP_ID=1 (John Doe)
    assert_eq!(rows[0][1], json!("John"));
    assert_eq!(rows[0][2], json!("Doe"));
}

#[test]
fn test_execute_query_pagination() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Page 1 with limit 3
    let page1 = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT EMP_ID FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID",
            "limit": 3,
            "page": 1,
        })),
    );
    let rows1 = page1["rows"].as_array().unwrap();
    assert_eq!(rows1.len(), 3, "page 1 should have 3 rows");
    assert_eq!(
        page1["pagination"]["has_more"],
        json!(true),
        "should have more pages"
    );

    // Page 2 with limit 3
    let page2 = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT EMP_ID FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID",
            "limit": 3,
            "page": 2,
        })),
    );
    let rows2 = page2["rows"].as_array().unwrap();
    assert_eq!(rows2.len(), 3, "page 2 should have 3 rows");

    // Pages should not overlap
    assert_ne!(rows1[0], rows2[0], "pages should return different data");
}

#[test]
fn test_execute_query_with_cte() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "WITH dept_counts AS (SELECT DEPT_ID, COUNT(*) AS cnt FROM TEST_SCHEMA.EMPLOYEES GROUP BY DEPT_ID) SELECT * FROM dept_counts ORDER BY cnt DESC",
            "limit": 10,
            "page": 1,
        })),
    );
    let rows = result["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "CTE query should return results");
}

// ---------------------------------------------------------------------------
// Tests — DDL generation (pure SQL generation, no DB2 needed)
// ---------------------------------------------------------------------------

#[test]
fn test_ddl_create_table() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_create_table_sql",
        json!({
            "table_name": "NEW_TABLE",
            "schema": "TEST_SCHEMA",
            "columns": [
                { "name": "ID", "data_type": "INTEGER", "is_pk": true, "is_nullable": false, "is_auto_increment": false, "default_value": null },
                { "name": "NAME", "data_type": "VARCHAR(100)", "is_pk": false, "is_nullable": true, "is_auto_increment": false, "default_value": null }
            ]
        }),
    );
    let stmts = result.as_array().expect("expected array");
    let sql = stmts[0].as_str().unwrap();
    assert!(sql.contains("CREATE TABLE"), "should contain CREATE TABLE");
    assert!(sql.contains("PRIMARY KEY"), "should contain PRIMARY KEY");
    assert!(sql.contains("\"TEST_SCHEMA\""), "should contain schema");
}

#[test]
fn test_ddl_add_column() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_add_column_sql",
        json!({
            "table": "EMPLOYEES",
            "schema": "TEST_SCHEMA",
            "column": {
                "name": "PHONE",
                "data_type": "VARCHAR(20)",
                "is_pk": false,
                "is_nullable": true,
                "is_auto_increment": false,
                "default_value": null
            }
        }),
    );
    let stmts = result.as_array().expect("expected array");
    let sql = stmts[0].as_str().unwrap();
    assert!(sql.contains("ADD COLUMN"), "should contain ADD COLUMN");
    assert!(sql.contains("PHONE"), "should reference PHONE");
}

#[test]
fn test_ddl_alter_column() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_alter_column_sql",
        json!({
            "table": "EMPLOYEES",
            "schema": "TEST_SCHEMA",
            "old_column": {
                "name": "NOTES",
                "data_type": "CLOB",
                "is_pk": false,
                "is_nullable": true,
                "is_auto_increment": false,
                "default_value": null
            },
            "new_column": {
                "name": "DESCRIPTION",
                "data_type": "VARCHAR(2000)",
                "is_pk": false,
                "is_nullable": true,
                "is_auto_increment": false,
                "default_value": null
            }
        }),
    );
    let stmts = result.as_array().expect("expected array");
    assert!(stmts.len() >= 2, "should produce RENAME + ALTER statements");
    let all_sql: String = stmts.iter().filter_map(Value::as_str).collect();
    assert!(all_sql.contains("RENAME COLUMN"), "should contain RENAME");
    assert!(
        all_sql.contains("SET DATA TYPE"),
        "should contain SET DATA TYPE"
    );
}

#[test]
fn test_ddl_create_index() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_create_index_sql",
        json!({
            "table": "EMPLOYEES",
            "schema": "TEST_SCHEMA",
            "index_name": "IDX_TEST",
            "columns": ["FIRST_NAME", "LAST_NAME"],
            "is_unique": true
        }),
    );
    let stmts = result.as_array().expect("expected array");
    let sql = stmts[0].as_str().unwrap();
    assert!(sql.contains("CREATE UNIQUE INDEX"), "should be unique");
    assert!(
        sql.contains("\"FIRST_NAME\"") && sql.contains("\"LAST_NAME\""),
        "should list columns"
    );
}

#[test]
fn test_ddl_create_foreign_key() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_create_foreign_key_sql",
        json!({
            "table": "PROJECTS",
            "schema": "TEST_SCHEMA",
            "fk_name": "FK_TEST",
            "column": "DEPT_ID",
            "ref_table": "DEPARTMENTS",
            "ref_column": "DEPT_ID",
            "on_delete": "CASCADE",
            "on_update": null
        }),
    );
    let stmts = result.as_array().expect("expected array");
    let sql = stmts[0].as_str().unwrap();
    assert!(sql.contains("FOREIGN KEY"), "should contain FOREIGN KEY");
    assert!(sql.contains("ON DELETE CASCADE"), "should set ON DELETE");
}

// ---------------------------------------------------------------------------
// Tests — CRUD operations (uses CRUD_TEST table)
// ---------------------------------------------------------------------------

#[test]
fn test_crud_insert_update_delete() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Insert
    let result = p.call_ok(
        "insert_record",
        p.schema_params(json!({
            "table": "CRUD_TEST",
            "data": {
                "ID": 100,
                "NAME": "Test Record",
                "VALUE": 42.50
            }
        })),
    );
    // insert_record returns affected rows count (0 due to odbc-api behavior)
    assert!(result.is_number() || result.is_null());

    // Verify via query
    let qr = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT ID, NAME, VALUE FROM TEST_SCHEMA.CRUD_TEST WHERE ID = 100",
            "limit": 10,
            "page": 1
        })),
    );
    let rows = qr["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "inserted row should exist");
    assert_eq!(rows[0][1], json!("Test Record"));

    // Update
    p.call_ok(
        "update_record",
        p.schema_params(json!({
            "table": "CRUD_TEST",
            "pk_col": "ID",
            "pk_val": 100,
            "col_name": "NAME",
            "new_val": "Updated Record"
        })),
    );

    // Verify update
    let qr = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT NAME FROM TEST_SCHEMA.CRUD_TEST WHERE ID = 100",
            "limit": 10,
            "page": 1
        })),
    );
    let rows = qr["rows"].as_array().unwrap();
    assert_eq!(rows[0][0], json!("Updated Record"));

    // Delete
    p.call_ok(
        "delete_record",
        p.schema_params(json!({
            "table": "CRUD_TEST",
            "pk_col": "ID",
            "pk_val": 100
        })),
    );

    // Verify delete
    let qr = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT COUNT(*) AS CNT FROM TEST_SCHEMA.CRUD_TEST WHERE ID = 100",
            "limit": 10,
            "page": 1
        })),
    );
    let rows = qr["rows"].as_array().unwrap();
    // COUNT returns 0
    let count = rows[0][0].as_str().unwrap_or("0");
    assert_eq!(count, "0", "row should be deleted");
}

// ---------------------------------------------------------------------------
// Tests — Drop operations (uses DROP_TEST table)
// ---------------------------------------------------------------------------

#[test]
fn test_drop_index() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Drop the test index
    let resp = p.call(
        "drop_index",
        p.schema_params(json!({
            "table": "DROP_TEST",
            "index_name": "IDX_DROP_TEST_NAME"
        })),
    );
    assert!(
        resp.get("result").is_some(),
        "drop_index failed: {resp}"
    );

    // Verify index is gone
    let result = p.call_ok(
        "get_indexes",
        p.schema_params(json!({ "table": "DROP_TEST" })),
    );
    let indexes = result.as_array().unwrap();
    let has_idx = indexes
        .iter()
        .any(|i| i["name"].as_str().map(|n| n.contains("IDX_DROP_TEST_NAME")).unwrap_or(false));
    assert!(!has_idx, "index should have been dropped");
}

#[test]
fn test_drop_foreign_key() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Drop the test FK
    let resp = p.call(
        "drop_foreign_key",
        p.schema_params(json!({
            "table": "DROP_TEST",
            "fk_name": "FK_DROP_TEST"
        })),
    );
    assert!(
        resp.get("result").is_some(),
        "drop_foreign_key failed: {resp}"
    );

    // Verify FK is gone
    let result = p.call_ok(
        "get_foreign_keys",
        p.schema_params(json!({ "table": "DROP_TEST" })),
    );
    let fks = result.as_array().unwrap();
    let has_fk = fks
        .iter()
        .any(|f| f["name"].as_str().map(|n| n.contains("FK_DROP_TEST")).unwrap_or(false));
    assert!(!has_fk, "FK should have been dropped");
}

// ---------------------------------------------------------------------------
// Tests — Explain query
// ---------------------------------------------------------------------------

#[test]
fn test_explain_query() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "explain_query",
        p.schema_params(json!({
            "query": "SELECT * FROM TEST_SCHEMA.EMPLOYEES WHERE DEPT_ID = 1",
            "analyze": false
        })),
    );

    assert_eq!(result["driver"], json!("db2"));
    assert_eq!(result["has_analyze_data"], json!(false));
    assert!(
        result["original_query"].as_str().unwrap().contains("EMPLOYEES"),
        "should preserve original query"
    );

    // The root node should have a real operator type from the explain tables,
    // not the old placeholder "DB2 Explain".
    let root = &result["root"];
    assert!(
        !root["id"].as_str().unwrap_or("").is_empty(),
        "root node should have an id"
    );
    assert!(
        !root["node_type"].as_str().unwrap_or("").is_empty(),
        "root node should have an operator type"
    );
    assert_ne!(
        root["node_type"].as_str().unwrap_or(""),
        "DB2 Explain",
        "root node should have a real operator type, not the placeholder"
    );

    // raw_output should contain the readable plan text, not the old placeholder.
    let raw = result["raw_output"].as_str().unwrap_or("");
    assert!(
        !raw.contains("not implemented yet"),
        "raw_output should contain real plan data"
    );
}

#[test]
fn test_explain_query_analyze() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "explain_query",
        p.schema_params(json!({
            "query": "SELECT * FROM TEST_SCHEMA.EMPLOYEES WHERE DEPT_ID = 1",
            "analyze": true
        })),
    );

    assert_eq!(result["driver"], json!("db2"));
    assert_eq!(result["has_analyze_data"], json!(true));
    assert!(
        result["original_query"].as_str().unwrap().contains("EMPLOYEES"),
        "should preserve original query"
    );
}

#[test]
fn test_explain_query_join() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "explain_query",
        p.schema_params(json!({
            "query": "SELECT e.FIRST_NAME, d.DEPT_NAME FROM TEST_SCHEMA.EMPLOYEES e JOIN TEST_SCHEMA.DEPARTMENTS d ON e.DEPT_ID = d.DEPT_ID",
            "analyze": false
        })),
    );

    assert_eq!(result["driver"], json!("db2"));

    // A JOIN query should produce a plan tree with children.
    let root = &result["root"];
    let raw = result["raw_output"].as_str().unwrap_or("");
    // The plan should mention multiple operators for a join.
    assert!(
        root["children"].as_array().map(|a| !a.is_empty()).unwrap_or(false)
            || raw.lines().count() > 1,
        "a JOIN query should produce a multi-node plan"
    );
}

// ---------------------------------------------------------------------------
// Tests — Error handling
// ---------------------------------------------------------------------------

#[test]
fn test_invalid_method() {
    require_db2!();
    let mut p = Plugin::spawn();

    let resp = p.call("nonexistent_method", json!({}));
    assert!(
        resp.get("error").is_some(),
        "unknown method should return error"
    );
}

#[test]
fn test_bad_connection() {
    require_db2!();
    let mut p = Plugin::spawn();

    let resp = p.call(
        "test_connection",
        json!({
            "params": {
                "driver": "db2",
                "host": "localhost",
                "port": 59999,
                "database": "NONEXISTENT",
                "username": "nobody",
                "password": "wrong"
            }
        }),
    );
    assert!(
        resp.get("error").is_some(),
        "bad connection should return error"
    );
}
