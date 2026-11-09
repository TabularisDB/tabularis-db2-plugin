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
        Self::spawn_with(|_| {})
    }

    /// Spawns the plugin after letting `configure` adjust its command line or
    /// environment.
    fn spawn_with(configure: impl FnOnce(&mut Command)) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tabularis-db2-plugin"));
        configure(&mut command);
        let mut child = command
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
        assert!(resp.get("result").is_some(), "initialize failed: {resp}");
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
        self.reader
            .read_line(&mut buf)
            .expect("failed to read response");
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

    /// Like `schema_params`, for a schema other than TEST_SCHEMA.
    fn params_in(&self, schema: &str, extras: Value) -> Value {
        let mut params = self.schema_params(extras);
        params["schema"] = json!(schema);
        params
    }

    /// `execute_query` in TEST_SCHEMA; `limit` may be `Value::Null`.
    fn query(&mut self, sql: &str, limit: Value, page: u32) -> Value {
        self.call(
            "execute_query",
            self.schema_params(json!({ "query": sql, "limit": limit, "page": page })),
        )
    }

    fn query_ok(&mut self, sql: &str, limit: Value, page: u32) -> Value {
        let resp = self.query(sql, limit, page);
        if let Some(err) = resp.get("error") {
            panic!("query failed: {err}\n{sql}");
        }
        resp["result"].clone()
    }

    /// Writes one raw line to the plugin without waiting for an answer.
    fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
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
    assert!(
        resp.get("result").is_some(),
        "test_connection failed: {resp}"
    );

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

    // User schemas are listed: one with objects, one that is empty, and one
    // whose name only exists as a delimited identifier.
    for expected in ["TEST_SCHEMA", "EMPTY_SCHEMA", "Mixed Case"] {
        assert!(
            names.contains(&expected),
            "{expected} not found in: {names:?}"
        );
    }

    // Schemas Db2 owns are not offered for browsing.
    for name in &names {
        assert!(
            !name.starts_with("SYS") && *name != "NULLID" && *name != "SQLJ",
            "system schema {name} should not be listed"
        );
    }

    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "schemas should be sorted by name");
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
    let names: Vec<&str> = tables.iter().filter_map(|t| t["name"].as_str()).collect();

    for expected in &[
        "DEPARTMENTS",
        "EMPLOYEES",
        "PROJECTS",
        "EMP_PROJECTS",
        "DATA_TYPES_TEST",
    ] {
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
fn test_get_tables_edge_cases() {
    require_db2!();
    let mut p = Plugin::spawn();

    // An empty schema has no tables, and that is not an error.
    let result = p.call_ok("get_tables", p.params_in("EMPTY_SCHEMA", json!({})));
    assert_eq!(result, json!([]));

    // Neither is a schema that does not exist.
    let result = p.call_ok("get_tables", p.params_in("NO_SUCH_SCHEMA", json!({})));
    assert_eq!(result, json!([]));

    // A delimited schema name with mixed case and a space, holding a table
    // whose name needs delimiters too.
    let result = p.call_ok("get_tables", p.params_in("Mixed Case", json!({})));
    assert_eq!(
        result,
        json!([{ "name": "Order Details", "comment": "Line items (delimited names)" }])
    );

    // Views are not tables: they are listed by get_views.
    let result = p.call_ok("get_tables", p.schema_params(json!({})));
    let names: Vec<&str> = result
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|table| table["name"].as_str())
        .collect();
    assert!(!names.contains(&"V_EMP_DETAILS"), "views listed as tables");
    assert!(names.contains(&"TYPE_METADATA_TEST"));
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
    let col_names: Vec<&str> = columns.iter().filter_map(|c| c["name"].as_str()).collect();
    for expected in &[
        "EMP_ID",
        "FIRST_NAME",
        "LAST_NAME",
        "EMAIL",
        "SALARY",
        "DEPT_ID",
    ] {
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

/// `[name, data_type, full_type, character_maximum_length, numeric_precision,
/// numeric_scale]` of a column, for compact comparisons.
fn type_summary(column: &Value) -> Value {
    json!([
        column["name"],
        column["data_type"],
        column.get("full_type").cloned().unwrap_or(Value::Null),
        column["character_maximum_length"],
        column
            .get("numeric_precision")
            .cloned()
            .unwrap_or(Value::Null),
        column.get("numeric_scale").cloned().unwrap_or(Value::Null),
    ])
}

#[test]
fn test_get_columns_type_metadata() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": "TYPE_METADATA_TEST" })),
    );
    let columns = result.as_array().expect("expected array");
    let summary: Vec<Value> = columns.iter().map(type_summary).collect();

    // Lengths only for string and binary types, precision and scale only for
    // decimals: the catalog's storage sizes (4 bytes for an INTEGER or a
    // DATE) must not leak out as lengths.
    assert_eq!(
        json!(summary),
        json!([
            ["ID", "BIGINT", null, null, null, null],
            ["COL_CHAR", "CHAR", "CHAR(10)", 10, null, null],
            ["COL_VARCHAR", "VARCHAR", "VARCHAR(255)", 255, null, null],
            [
                "COL_BIT_DATA",
                "VARCHAR",
                "VARCHAR(16) FOR BIT DATA",
                16,
                null,
                null
            ],
            ["COL_GRAPHIC", "GRAPHIC", "GRAPHIC(4)", 4, null, null],
            [
                "COL_VARGRAPHIC",
                "VARGRAPHIC",
                "VARGRAPHIC(40)",
                40,
                null,
                null
            ],
            ["COL_SMALLINT", "SMALLINT", null, null, null, null],
            ["COL_INTEGER", "INTEGER", null, null, null, null],
            ["COL_DECIMAL", "DECIMAL", "DECIMAL(18,4)", null, 18, 4],
            ["COL_DECFLOAT16", "DECFLOAT", "DECFLOAT(16)", null, 16, null],
            ["COL_DECFLOAT34", "DECFLOAT", "DECFLOAT(34)", null, 34, null],
            ["COL_REAL", "REAL", null, null, null, null],
            ["COL_DOUBLE", "DOUBLE", null, null, null, null],
            ["COL_BOOLEAN", "BOOLEAN", null, null, null, null],
            ["COL_DATE", "DATE", null, null, null, null],
            ["COL_TIME", "TIME", null, null, null, null],
            ["COL_TIMESTAMP", "TIMESTAMP", null, null, null, null],
            [
                "COL_TIMESTAMP12",
                "TIMESTAMP",
                "TIMESTAMP(12)",
                null,
                null,
                null
            ],
            ["COL_CLOB", "CLOB", "CLOB(1M)", 1048576, null, null],
            ["COL_DBCLOB", "DBCLOB", "DBCLOB(64K)", 65536, null, null],
            ["COL_BLOB", "BLOB", "BLOB(2M)", 2097152, null, null],
            ["COL_BINARY", "BINARY", "BINARY(16)", 16, null, null],
            [
                "COL_VARBINARY",
                "VARBINARY",
                "VARBINARY(64)",
                64,
                null,
                null
            ],
            ["COL_XML", "XML", null, null, null, null],
            ["COL_COMPUTED", "INTEGER", null, null, null, null],
        ]),
        // COL_HIDDEN is implicitly hidden: SELECT * does not return it, so
        // the metadata does not list it either.
    );

    let column = |name: &str| {
        columns
            .iter()
            .find(|column| column["name"] == name)
            .unwrap_or_else(|| panic!("{name} column missing"))
    };

    // Identity: auto-increment, and not "generated" (a value may be given).
    assert_eq!(column("ID")["is_pk"], json!(true));
    assert_eq!(column("ID")["is_auto_increment"], json!(true));
    assert_eq!(column("ID")["is_generated"], json!(false));
    assert_eq!(column("ID")["is_nullable"], json!(false));

    // A GENERATED ALWAYS AS (expression) column is computed by Db2.
    assert_eq!(column("COL_COMPUTED")["is_generated"], json!(true));
    assert_eq!(column("COL_COMPUTED")["is_auto_increment"], json!(false));

    // Defaults come back as SQL, usable after DEFAULT as they are.
    for (name, default) in [
        ("COL_CHAR", "'N/A'"),
        ("COL_SMALLINT", "1"),
        ("COL_DECIMAL", "0"),
        ("COL_BOOLEAN", "FALSE"),
        ("COL_DATE", "CURRENT DATE"),
        ("COL_TIMESTAMP", "CURRENT TIMESTAMP"),
    ] {
        assert_eq!(column(name)["default_value"], json!(default), "{name}");
        assert_eq!(column(name)["default_sql"], json!(default), "{name}");
    }
    assert_eq!(column("COL_VARCHAR")["default_value"], Value::Null);
    assert!(column("COL_VARCHAR").get("default_sql").is_none());

    assert_eq!(column("COL_CHAR")["is_nullable"], json!(false));
    assert_eq!(column("COL_VARCHAR")["is_nullable"], json!(true));
    assert_eq!(
        column("COL_DECIMAL")["comment"],
        json!("Amount, 4 decimals")
    );
}

#[test]
fn test_get_columns_special_names() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_columns",
        p.params_in("Mixed Case", json!({ "table": "Order Details" })),
    );
    let names: Vec<&str> = result
        .as_array()
        .expect("expected array")
        .iter()
        .filter_map(|column| column["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Order Id",
            "Unit Price",
            "select",
            "Qty#",
            "O'Brien",
            "Größe"
        ]
    );
    assert_eq!(result[0]["is_pk"], json!(true));
    assert_eq!(result[1]["default_value"], json!("0"));

    // A view in that schema, through get_view_columns.
    let result = p.call_ok(
        "get_view_columns",
        p.params_in("Mixed Case", json!({ "view_name": "Cheap Orders" })),
    );
    let summary: Vec<Value> = result
        .as_array()
        .unwrap()
        .iter()
        .map(type_summary)
        .collect();
    assert_eq!(
        json!(summary),
        json!([
            ["Order Id", "INTEGER", null, null, null, null],
            ["Unit Price", "DECIMAL", "DECIMAL(9,2)", null, 9, 2],
        ])
    );

    // A quote in the names is data, not SQL: no error, no match.
    let result = p.call_ok(
        "get_columns",
        p.params_in("Mixed' Case", json!({ "table": "Order' Details" })),
    );
    assert_eq!(result, json!([]));

    // A table that does not exist has no columns.
    let result = p.call_ok("get_columns", p.schema_params(json!({ "table": "NOPE" })));
    assert_eq!(result, json!([]));

    // The table name is required.
    let resp = p.call("get_columns", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");
}

#[test]
fn test_get_all_columns_batch_matches_get_columns() {
    require_db2!();
    let mut p = Plugin::spawn();

    let batch = p.call_ok("get_all_columns_batch", p.schema_params(json!({})));
    let map = batch.as_object().expect("expected object");

    // Every table and view of the schema, each with the same columns
    // get_columns reports for it.
    for table in [
        "EMPLOYEES",
        "TYPE_METADATA_TEST",
        "EMP_PROJECTS",
        "V_EMP_DETAILS",
    ] {
        let single = p.call_ok("get_columns", p.schema_params(json!({ "table": table })));
        assert_eq!(map.get(table), Some(&single), "{table}");
    }

    // `tables` narrows the batch.
    let batch = p.call_ok(
        "get_all_columns_batch",
        p.schema_params(json!({ "tables": ["EMPLOYEES", "DEPARTMENTS", "NOPE"] })),
    );
    let mut keys: Vec<&String> = batch.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["DEPARTMENTS", "EMPLOYEES"]);

    // An empty schema gives an empty map.
    let batch = p.call_ok(
        "get_all_columns_batch",
        p.params_in("EMPTY_SCHEMA", json!({})),
    );
    assert_eq!(batch, json!({}));
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
    let names: Vec<&str> = views.iter().filter_map(|v| v["name"].as_str()).collect();

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
    let names: Vec<&str> = columns.iter().filter_map(|c| c["name"].as_str()).collect();

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
        .find(|f| {
            f["name"]
                .as_str()
                .map(|n| n.contains("FK_EMP_DEPT"))
                .unwrap_or(false)
        })
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
    let names: Vec<&str> = indexes.iter().filter_map(|i| i["name"].as_str()).collect();

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
    let names: Vec<&str> = routines.iter().filter_map(|r| r["name"].as_str()).collect();

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

    let modes: Vec<&str> = params.iter().filter_map(|p| p["mode"].as_str()).collect();
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

    let names: Vec<&str> = tables.iter().filter_map(|t| t["name"].as_str()).collect();
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

#[test]
fn test_get_schema_snapshot_edge_cases() {
    require_db2!();
    let mut p = Plugin::spawn();

    let snapshot = p.call_ok(
        "get_schema_snapshot",
        p.params_in("EMPTY_SCHEMA", json!({})),
    );
    assert_eq!(snapshot, json!([]));

    let snapshot = p.call_ok("get_schema_snapshot", p.params_in("Mixed Case", json!({})));
    let tables = snapshot.as_array().expect("expected array");
    assert_eq!(tables.len(), 1, "views are not part of the snapshot");
    assert_eq!(tables[0]["name"], json!("Order Details"));
    assert_eq!(tables[0]["columns"].as_array().unwrap().len(), 6);
    assert_eq!(tables[0]["foreign_keys"], json!([]));

    // The snapshot is the per-table metadata, table by table.
    let snapshot = p.call_ok("get_schema_snapshot", p.schema_params(json!({})));
    let tables = snapshot.as_array().unwrap();
    let listed = p.call_ok("get_tables", p.schema_params(json!({})));
    let listed: Vec<&Value> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| &t["name"])
        .collect();
    let in_snapshot: Vec<&Value> = tables.iter().map(|t| &t["name"]).collect();
    assert_eq!(in_snapshot, listed);

    for table in tables {
        let name = table["name"].as_str().unwrap();
        let columns = p.call_ok("get_columns", p.schema_params(json!({ "table": name })));
        assert_eq!(table["columns"], columns, "{name}");
    }
}

// ---------------------------------------------------------------------------
// Tests — Query execution
// ---------------------------------------------------------------------------

#[test]
fn test_execute_query() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.query_ok(
        "SELECT EMP_ID, FIRST_NAME, LAST_NAME, SALARY FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID",
        json!(10),
        1,
    );

    assert_eq!(
        result["columns"],
        json!(["EMP_ID", "FIRST_NAME", "LAST_NAME", "SALARY"])
    );
    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 6, "should return all 6 employees");

    // Integers are JSON numbers, text is text, and a DECIMAL keeps its
    // digits as text.
    assert_eq!(rows[0], json!([1, "John", "Doe", "75000.00"]));
    assert_eq!(result["affected_rows"], json!(0));
    assert_eq!(result["truncated"], json!(false));
    assert_eq!(
        result["pagination"],
        json!({ "page": 1, "page_size": 10, "total_rows": null, "has_more": false })
    );
}

#[test]
fn test_execute_query_value_types() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.query_ok(
        "SELECT * FROM TEST_SCHEMA.TYPE_METADATA_TEST ORDER BY ID",
        json!(10),
        1,
    );
    let columns = result["columns"].as_array().unwrap();
    let rows = result["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);

    let value = |row: usize, name: &str| {
        let index = columns
            .iter()
            .position(|column| column == name)
            .unwrap_or_else(|| panic!("{name} missing from {columns:?}"));
        rows[row][index].clone()
    };

    for (name, expected) in [
        ("ID", json!(1)),
        ("COL_CHAR", json!("abc       ")),
        ("COL_VARCHAR", json!("héllo wörld 所有")),
        ("COL_BIT_DATA", json!("CAFEBABE")),
        ("COL_GRAPHIC", json!("グラ  ")),
        ("COL_VARGRAPHIC", json!("变长")),
        ("COL_SMALLINT", json!(7)),
        ("COL_INTEGER", json!(-42)),
        ("COL_DECIMAL", json!("1234.5678")),
        ("COL_DECFLOAT16", json!("1.5")),
        ("COL_DECFLOAT34", json!("12345678901234567890.123456789")),
        ("COL_REAL", json!(1.25)),
        ("COL_DOUBLE", json!(75000.5)),
        ("COL_BOOLEAN", json!(true)),
        ("COL_DATE", json!("2024-02-29")),
        ("COL_TIME", json!("13:45:10")),
        ("COL_TIMESTAMP", json!("2024-02-29 13:45:10.123456")),
        ("COL_TIMESTAMP12", json!("2024-02-29 13:45:10.123456789012")),
        ("COL_CLOB", json!("clob text")),
        ("COL_DBCLOB", json!("dbclob 文本")),
        // 10 bytes that start with the PNG signature, in the host's blob
        // wire format.
        ("COL_BLOB", json!("BLOB:10:image/png:iVBORw0KGgoAAA==")),
        ("COL_BINARY", json!("00112233445566778899AABBCCDDEEFF")),
        ("COL_VARBINARY", json!("DEADBEEF")),
        ("COL_COMPUTED", json!(20)),
    ] {
        assert_eq!(value(0, name), expected, "{name}");
    }
    let xml = value(0, "COL_XML");
    assert!(xml.as_str().unwrap().ends_with("<a><b>1</b></a>"), "{xml}");

    // The implicitly hidden column is not part of SELECT *.
    assert!(!columns.contains(&json!("COL_HIDDEN")));

    // SQL NULL is JSON null whatever the column type.
    for name in [
        "COL_VARCHAR",
        "COL_INTEGER",
        "COL_DOUBLE",
        "COL_TIME",
        "COL_TIMESTAMP12",
        "COL_CLOB",
        "COL_BLOB",
        "COL_VARBINARY",
        "COL_XML",
        "COL_COMPUTED",
    ] {
        assert_eq!(value(1, name), Value::Null, "{name}");
    }
    // Columns with a default got it instead.
    assert_eq!(value(1, "COL_BOOLEAN"), json!(false));
    assert_eq!(value(1, "COL_DECIMAL"), json!("0.0000"));
    assert_eq!(value(1, "COL_SMALLINT"), json!(1));
}

#[test]
fn test_unicode_survives_the_c_locale() {
    require_db2!();
    // What a container or a service runs with. The Db2 CLI driver would pick
    // a single-byte code page from it and replace every other character.
    let mut p = Plugin::spawn_with(|command| {
        command
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env_remove("DB2CODEPAGE");
    });

    let result = p.query_ok(
        "SELECT COL_VARCHAR, COL_VARGRAPHIC, COL_DBCLOB FROM TEST_SCHEMA.TYPE_METADATA_TEST WHERE ID = 1",
        json!(1),
        1,
    );
    assert_eq!(
        result["rows"],
        json!([["héllo wörld 所有", "变长", "dbclob 文本"]])
    );

    // Non-ASCII text in the statement itself, and in the catalog.
    let echo = p.query_ok("VALUES 'Größe ✓ 所有员工'", json!(1), 1);
    assert_eq!(echo["rows"], json!([["Größe ✓ 所有员工"]]));

    let tables = p.call_ok("get_tables", p.schema_params(json!({})));
    let employees = tables
        .as_array()
        .unwrap()
        .iter()
        .find(|table| table["name"] == "EMPLOYEES")
        .expect("EMPLOYEES table missing");
    assert_eq!(employees["comment"], json!(TABLE_COMMENT));

    let columns = p.call_ok(
        "get_columns",
        p.params_in("Mixed Case", json!({ "table": "Order Details" })),
    );
    assert_eq!(columns[5]["name"], json!("Größe"));
}

#[test]
fn test_execute_query_large_blob_is_previewed() {
    require_db2!();
    let mut p = Plugin::spawn();

    // 50,000 bytes: more than the preview carries. The header reports the
    // true size; the payload is the first 10,240 bytes.
    let result = p.query_ok(
        "SELECT BLOB(REPEAT('A', 30000)) || BLOB(REPEAT('B', 20000)) AS BIG FROM SYSIBM.SYSDUMMY1",
        json!(1),
        1,
    );
    let wire = result["rows"][0][0].as_str().expect("blob wire format");
    let prefix = "BLOB:50000:application/octet-stream:";
    assert!(wire.starts_with(prefix), "{}", &wire[..60]);
    let payload = &wire[prefix.len()..];
    assert_eq!(payload.len(), 10_240_usize.div_ceil(3) * 4);
}

#[test]
fn test_execute_query_statement_kinds() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Whether a statement returns rows is decided by Db2, not by its first
    // keyword.
    let values = p.query_ok("VALUES (1, 'a'), (2, 'b')", json!(10), 1);
    assert_eq!(values["rows"], json!([[1, "a"], [2, "b"]]));

    let call = p.query_ok("CALL TEST_SCHEMA.GET_EMPLOYEES_BY_DEPT(1)", json!(10), 1);
    assert_eq!(call["columns"][0], json!("EMP_ID"));
    assert_eq!(call["rows"].as_array().unwrap().len(), 3);

    let commented = p.query_ok(
        "-- how many?\nSELECT COUNT(*) AS N FROM TEST_SCHEMA.EMPLOYEES;\n",
        json!(10),
        1,
    );
    assert_eq!(commented["rows"], json!([[6]]));

    // Unqualified names resolve in the selected schema: tables through
    // CURRENT SCHEMA, routines through CURRENT PATH.
    let unqualified = p.query_ok(
        "SELECT FULL_NAME(FIRST_NAME, LAST_NAME) FROM EMPLOYEES WHERE EMP_ID = 1",
        json!(10),
        1,
    );
    assert_eq!(unqualified["rows"], json!([["John Doe"]]));

    // Columns that share a name are both returned.
    let duplicates = p.query_ok(
        "SELECT E.DEPT_ID, D.DEPT_ID FROM EMPLOYEES E JOIN DEPARTMENTS D ON E.DEPT_ID = D.DEPT_ID WHERE E.EMP_ID = 3",
        json!(10),
        1,
    );
    assert_eq!(duplicates["columns"], json!(["DEPT_ID", "DEPT_ID"]));
    assert_eq!(duplicates["rows"], json!([[2, 2]]));

    // What Tabularis runs to count the rows of a grid query: the count must
    // be a JSON integer, and no limit means no pagination block.
    let count = p.query_ok(
        "SELECT COUNT(*) FROM (SELECT * FROM \"TEST_SCHEMA\".\"EMPLOYEES\") as count_wrapper",
        Value::Null,
        1,
    );
    assert_eq!(count["rows"][0][0].as_i64(), Some(6));
    assert_eq!(count["pagination"], Value::Null);
}

#[test]
fn test_execute_query_affected_rows() {
    require_db2!();
    let mut p = Plugin::spawn();
    p.query_ok(
        "DELETE FROM CRUD_TEST WHERE ID BETWEEN 700 AND 799",
        Value::Null,
        1,
    );

    let insert = p.query_ok(
        "INSERT INTO CRUD_TEST (ID, NAME, VALUE) VALUES (701, 'a', 1.5), (702, 'b', 2.5), (703, 'c', 3.5)",
        json!(100),
        1,
    );
    assert_eq!(insert["affected_rows"], json!(3));
    assert_eq!(insert["columns"], json!([]));
    assert_eq!(insert["rows"], json!([]));
    assert_eq!(insert["pagination"], Value::Null);

    let update = p.query_ok(
        "UPDATE CRUD_TEST SET NAME = 'z' WHERE ID IN (701, 702)",
        json!(100),
        1,
    );
    assert_eq!(update["affected_rows"], json!(2));

    // A statement that matches nothing succeeds with a count of 0.
    let nothing = p.query_ok(
        "UPDATE CRUD_TEST SET NAME = 'z' WHERE ID = -1",
        json!(100),
        1,
    );
    assert_eq!(nothing["affected_rows"], json!(0));

    let delete = p.query_ok(
        "DELETE FROM CRUD_TEST WHERE ID BETWEEN 700 AND 799",
        json!(100),
        1,
    );
    assert_eq!(delete["affected_rows"], json!(3));

    // DDL and compound statements report no rows rather than a bogus count.
    let create = p.query_ok(
        "CREATE TABLE TEST_SCHEMA.TMP_AFFECTED (A INT)",
        json!(100),
        1,
    );
    assert_eq!(create["affected_rows"], json!(0));
    let compound = p.query_ok(
        "BEGIN DECLARE X INT; SET X = 1; INSERT INTO TEST_SCHEMA.TMP_AFFECTED VALUES (X); END;",
        json!(100),
        1,
    );
    assert_eq!(compound["affected_rows"], json!(0));
    let kept = p.query_ok("SELECT A FROM TEST_SCHEMA.TMP_AFFECTED", json!(100), 1);
    assert_eq!(
        kept["rows"],
        json!([[1]]),
        "the compound statement ran as one"
    );
    p.query_ok("DROP TABLE TEST_SCHEMA.TMP_AFFECTED", json!(100), 1);
}

#[test]
fn test_execute_query_batch() {
    require_db2!();
    let mut p = Plugin::spawn();
    p.query("DROP TABLE TEST_SCHEMA.BATCH_TMP", Value::Null, 1);

    let results = p.call_ok(
        "execute_query_batch",
        p.schema_params(json!({
            "queries": [
                "CREATE TABLE BATCH_TMP (N INTEGER)",
                "INSERT INTO BATCH_TMP VALUES (1), (2), (3)",
                "SELEC broken",
                "SELECT N FROM BATCH_TMP ORDER BY N",
                // A compound statement is one statement.
                "BEGIN DECLARE V INTEGER; SET V = 10; UPDATE BATCH_TMP SET N = N + V; END",
                "SELECT SUM(N) AS TOTAL FROM BATCH_TMP;",
                // Session state: it only reaches the next statement if the
                // whole script runs on one connection.
                "SET CURRENT SCHEMA = 'EMPTY_SCHEMA'",
                "VALUES CURRENT SCHEMA",
                "DROP TABLE TEST_SCHEMA.BATCH_TMP",
            ],
            "limit": 2,
            "page": 1,
        })),
    );
    let results = results.as_array().expect("one entry per statement");
    assert_eq!(results.len(), 9);

    for (index, entry) in results.iter().enumerate() {
        assert!(entry["execution_time_ms"].is_number(), "statement {index}");
        assert_ne!(
            entry["result"].is_null(),
            entry["error"].is_null(),
            "exactly one of result and error: {entry}"
        );
    }

    assert_eq!(results[0]["result"]["affected_rows"], json!(0));
    assert_eq!(results[1]["result"]["affected_rows"], json!(3));

    // The failing statement reports its error and the script carries on.
    let error = results[2]["error"].as_str().expect("error text");
    assert!(error.starts_with("SQL0104N"), "{error}");

    // Paged like a single query: two of the three rows, more to come.
    assert_eq!(results[3]["result"]["rows"], json!([[1], [2]]));
    assert_eq!(results[3]["result"]["pagination"]["has_more"], json!(true));

    assert_eq!(results[4]["result"]["affected_rows"], json!(0));
    assert_eq!(results[5]["result"]["rows"], json!([[36]]));

    assert_eq!(results[7]["result"]["rows"], json!([["EMPTY_SCHEMA"]]));
    assert!(results[8]["error"].is_null(), "{}", results[8]);

    // An empty script is an empty list; `queries` itself is required.
    let empty = p.call_ok(
        "execute_query_batch",
        p.schema_params(json!({ "queries": [], "limit": 10, "page": 1 })),
    );
    assert_eq!(empty, json!([]));
    let resp = p.call("execute_query_batch", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");
}

#[test]
fn test_server_time_probe() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The statement Tabularis sends for "Set to server NOW".
    let now = p.query_ok("SELECT NOW()", json!(1), 1);
    let value = now["rows"][0][0].as_str().expect("timestamp text");
    assert_eq!(value.len(), "2026-01-01 00:00:00.000000".len(), "{value}");
    assert_eq!(&value[4..5], "-");
    assert_eq!(&value[10..11], " ");
}

#[test]
fn test_execute_query_errors() {
    require_db2!();
    let mut p = Plugin::spawn();

    let message = |resp: &Value| {
        resp["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {resp}"))
            .to_string()
    };

    // Invalid SQL: the Db2 message with its SQLSTATE, nothing else.
    let syntax = message(&p.query("SELEC * FROM EMPLOYEES", json!(10), 1));
    assert!(
        syntax.starts_with("SQL0104N An unexpected token \"SELEC\""),
        "{syntax}"
    );
    assert!(syntax.ends_with("SQLSTATE=42601"), "{syntax}");

    let missing = message(&p.query("SELECT * FROM NOPE", json!(10), 1));
    assert_eq!(
        missing,
        "SQL0204N \"TEST_SCHEMA.NOPE\" is an undefined name. SQLSTATE=42704"
    );

    let column = message(&p.query("SELECT NOPE FROM EMPLOYEES", json!(10), 1));
    assert!(
        column.starts_with("SQL0206N \"NOPE\" is not valid"),
        "{column}"
    );

    let duplicate = message(&p.query(
        "INSERT INTO EMP_PROJECTS (EMP_ID, PROJECT_ID) VALUES (1, 1)",
        json!(10),
        1,
    ));
    assert!(duplicate.starts_with("SQL0803N"), "{duplicate}");
    assert!(duplicate.contains("SQLSTATE=23505"), "{duplicate}");

    let division = message(&p.query("SELECT 1 / (EMP_ID - EMP_ID) FROM EMPLOYEES", json!(10), 1));
    assert!(division.contains("SQLSTATE=22012"), "{division}");

    assert_eq!(
        message(&p.query("  ;  ", json!(10), 1)),
        "The query is empty"
    );

    // The plugin keeps answering after an error.
    let after = p.query_ok("VALUES 1", json!(10), 1);
    assert_eq!(after["rows"], json!([[1]]));
}

#[test]
fn test_execute_query_pagination() {
    require_db2!();
    let mut p = Plugin::spawn();
    let sql = "SELECT EMP_ID FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID";
    let ids = |result: &Value| -> Vec<i64> {
        result["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[0].as_i64().unwrap())
            .collect()
    };

    // Six employees in pages of four.
    let page1 = p.query_ok(sql, json!(4), 1);
    assert_eq!(ids(&page1), [1, 2, 3, 4]);
    assert_eq!(
        page1["pagination"],
        json!({ "page": 1, "page_size": 4, "total_rows": null, "has_more": true })
    );
    assert_eq!(page1["truncated"], json!(true));

    let page2 = p.query_ok(sql, json!(4), 2);
    assert_eq!(ids(&page2), [5, 6]);
    assert_eq!(page2["pagination"]["has_more"], json!(false));
    assert_eq!(page2["truncated"], json!(false));

    // A page past the end is empty, with the columns still described.
    let page3 = p.query_ok(sql, json!(4), 3);
    assert_eq!(ids(&page3), Vec::<i64>::new());
    assert_eq!(page3["columns"], json!(["EMP_ID"]));
    assert_eq!(page3["pagination"]["has_more"], json!(false));

    // A page that ends exactly on the last row does not claim more.
    let exact = p.query_ok(sql, json!(6), 1);
    assert_eq!(ids(&exact), [1, 2, 3, 4, 5, 6]);
    assert_eq!(exact["pagination"]["has_more"], json!(false));
    let halves = p.query_ok(sql, json!(3), 2);
    assert_eq!(ids(&halves), [4, 5, 6]);
    assert_eq!(halves["pagination"]["has_more"], json!(false));

    // Every row is returned exactly once across the pages.
    let mut seen = Vec::new();
    for page in 1..=6 {
        seen.extend(ids(&p.query_ok(sql, json!(1), page)));
    }
    assert_eq!(seen, [1, 2, 3, 4, 5, 6]);

    // Page 0 is the first page.
    assert_eq!(ids(&p.query_ok(sql, json!(2), 0)), [1, 2]);

    // Without a limit everything comes back and there is no pagination.
    let all = p.query_ok(sql, Value::Null, 1);
    assert_eq!(ids(&all), [1, 2, 3, 4, 5, 6]);
    assert_eq!(all["pagination"], Value::Null);

    // The statement runs as written: its own row limit and isolation clause
    // stay in force, and paging applies to what it returns.
    let limited = "SELECT EMP_ID FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID DESC FETCH FIRST 3 ROWS ONLY WITH UR";
    assert_eq!(ids(&p.query_ok(limited, json!(2), 1)), [6, 5]);
    let rest = p.query_ok(limited, json!(2), 2);
    assert_eq!(ids(&rest), [4]);
    assert_eq!(rest["pagination"]["has_more"], json!(false));

    // Paging works for statements that are not a SELECT, too.
    let values = p.query_ok("VALUES 1, 2, 3, 4, 5", json!(2), 2);
    assert_eq!(ids(&values), [3, 4]);
    assert_eq!(values["pagination"]["has_more"], json!(true));
}

#[test]
fn test_execute_query_with_cte() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.query_ok(
        "WITH dept_counts AS (SELECT DEPT_ID, COUNT(*) AS cnt FROM TEST_SCHEMA.EMPLOYEES GROUP BY DEPT_ID) SELECT * FROM dept_counts ORDER BY DEPT_ID",
        json!(10),
        1,
    );
    assert_eq!(result["columns"], json!(["DEPT_ID", "CNT"]));
    assert_eq!(result["rows"], json!([[1, 3], [2, 1], [3, 1], [4, 1]]));
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
    assert_eq!(qr["rows"], json!([[0]]), "row should be deleted");
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
    assert!(resp.get("result").is_some(), "drop_index failed: {resp}");

    // Verify index is gone
    let result = p.call_ok(
        "get_indexes",
        p.schema_params(json!({ "table": "DROP_TEST" })),
    );
    let indexes = result.as_array().unwrap();
    let has_idx = indexes.iter().any(|i| {
        i["name"]
            .as_str()
            .map(|n| n.contains("IDX_DROP_TEST_NAME"))
            .unwrap_or(false)
    });
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
    let has_fk = fks.iter().any(|f| {
        f["name"]
            .as_str()
            .map(|n| n.contains("FK_DROP_TEST"))
            .unwrap_or(false)
    });
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
        result["original_query"]
            .as_str()
            .unwrap()
            .contains("EMPLOYEES"),
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
        result["original_query"]
            .as_str()
            .unwrap()
            .contains("EMPLOYEES"),
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
        root["children"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
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

    // Tabularis falls back to its own behaviour for optional methods on
    // -32601 (and, in older versions, on the "Method not found" text).
    let resp = p.call("nonexistent_method", json!({}));
    assert_eq!(resp["error"]["code"], json!(-32601), "{resp}");
    assert_eq!(
        resp["error"]["message"],
        json!("Method not found: nonexistent_method")
    );
}

#[test]
fn test_notifications_are_not_answered() {
    require_db2!();
    let mut p = Plugin::spawn();

    // A request without an id (Tabularis sends `cancel` this way) must not
    // produce a line on stdout: the next answer belongs to the next request.
    p.send_raw(r#"{"jsonrpc":"2.0","method":"cancel","params":{"id":1}}"#);
    let resp = p.call("ping", p.base_params());
    assert!(resp.get("result").is_some(), "{resp}");
    assert_eq!(resp["id"], json!(p.next_id - 1));
}

#[test]
fn test_long_values_are_not_truncated() {
    require_db2!();
    let mut p = Plugin::spawn();

    // 28,000 bytes of multi-byte text: longer than one fetch buffer, with
    // characters that straddle the buffer boundaries.
    let qr = p.call_ok(
        "execute_query",
        p.schema_params(json!({
            "query": "SELECT CAST(REPEAT('€uro-', 4000) AS CLOB(1M)) AS BIG, 'tail' AS AFTER_IT FROM SYSIBM.SYSDUMMY1",
            "limit": 10,
            "page": 1
        })),
    );
    let row = &qr["rows"][0];
    let big = row[0].as_str().expect("text value");
    assert_eq!(big.chars().count(), 20_000);
    assert_eq!(big, "€uro-".repeat(4000));
    // The column after the long one is still read correctly.
    assert_eq!(row[1], json!("tail"));
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
    let message = resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("bad connection should return error: {resp}"));
    // The Db2 message, without the ODBC call name and driver tags around it.
    assert!(message.starts_with("SQL30081N"), "{message}");
    assert!(message.contains("SQLSTATE=08001"), "{message}");
    assert!(!message.contains("ODBC emitted"), "{message}");

    // A wrong password on a reachable server.
    let mut params = conn_params();
    params["password"] = json!("definitely-wrong");
    let resp = p.call("test_connection", json!({ "params": params }));
    let message = resp["error"]["message"].as_str().expect("error expected");
    assert!(message.starts_with("SQL30082N"), "{message}");

    // Another driver's connection is rejected before connecting.
    let mut params = conn_params();
    params["driver"] = json!("postgres");
    let resp = p.call("test_connection", json!({ "params": params }));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");
}
