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
    // Every view of the schema, sorted, and nothing that is not a view.
    assert_eq!(names, ["V_DEPT_SUMMARY", "V_EMP_DETAILS"]);

    // The definition is the view's query, without the CREATE VIEW in front.
    for view in views {
        let definition = view["definition"].as_str().expect("definition");
        assert!(definition.starts_with("SELECT"), "{definition}");
    }

    // A schema without views, and one with delimited names.
    let none = p.call_ok("get_views", p.params_in("EMPTY_SCHEMA", json!({})));
    assert_eq!(none, json!([]));
    let mixed = p.call_ok("get_views", p.params_in("Mixed Case", json!({})));
    assert_eq!(mixed.as_array().unwrap().len(), 1);
    assert_eq!(mixed[0]["name"], json!("Cheap Orders"));
    assert_eq!(
        mixed[0]["definition"],
        json!("SELECT \"Order Id\", \"Unit Price\" FROM \"Mixed Case\".\"Order Details\" WHERE \"Unit Price\" < 10")
    );
}

#[test]
fn test_get_view_definition() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The query as Db2 stored it (the command line processor that loads the
    // fixtures joins the lines), without the CREATE VIEW in front.
    let result = p.call_ok(
        "get_view_definition",
        p.schema_params(json!({ "view_name": "V_EMP_DETAILS" })),
    );
    assert_eq!(
        result,
        json!(
            "SELECT e.EMP_ID, e.FIRST_NAME, e.LAST_NAME, e.EMAIL, e.HIRE_DATE, e.SALARY, \
             d.DEPT_NAME, d.LOCATION FROM TEST_SCHEMA.EMPLOYEES e LEFT JOIN \
             TEST_SCHEMA.DEPARTMENTS d ON e.DEPT_ID = d.DEPT_ID"
        )
    );

    // The definition is itself a query that returns what the view returns.
    let definition = result.as_str().unwrap().to_string();
    let from_definition = p.query_ok(&definition, Value::Null, 1);
    let from_view = p.query_ok("SELECT * FROM TEST_SCHEMA.V_EMP_DETAILS", Value::Null, 1);
    assert_eq!(from_definition["columns"], from_view["columns"]);
    assert_eq!(
        from_definition["rows"].as_array().unwrap().len(),
        from_view["rows"].as_array().unwrap().len()
    );

    // Both spellings of the argument, and the errors.
    let by_guide_name = p.call_ok(
        "get_view_definition",
        p.schema_params(json!({ "view": "V_DEPT_SUMMARY" })),
    );
    assert!(by_guide_name
        .as_str()
        .unwrap()
        .contains("COUNT(e.EMP_ID) AS EMP_COUNT"));

    let resp = p.call(
        "get_view_definition",
        p.schema_params(json!({ "view_name": "NOPE" })),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("View \"TEST_SCHEMA\".\"NOPE\" was not found")
    );
    // A table is not a view.
    let resp = p.call(
        "get_view_definition",
        p.schema_params(json!({ "view_name": "EMPLOYEES" })),
    );
    assert!(resp.get("error").is_some(), "{resp}");
    let resp = p.call("get_view_definition", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");
}

#[test]
fn test_get_view_columns() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok(
        "get_view_columns",
        p.schema_params(json!({ "view_name": "V_EMP_DETAILS" })),
    );
    let summary: Vec<Value> = result
        .as_array()
        .unwrap()
        .iter()
        .map(type_summary)
        .collect();
    // The columns of the view, in order, with the types of the columns they
    // come from.
    assert_eq!(
        json!(summary),
        json!([
            ["EMP_ID", "INTEGER", null, null, null, null],
            ["FIRST_NAME", "VARCHAR", "VARCHAR(50)", 50, null, null],
            ["LAST_NAME", "VARCHAR", "VARCHAR(50)", 50, null, null],
            ["EMAIL", "VARCHAR", "VARCHAR(100)", 100, null, null],
            ["HIRE_DATE", "DATE", null, null, null, null],
            ["SALARY", "DECIMAL", "DECIMAL(10,2)", null, 10, 2],
            ["DEPT_NAME", "VARCHAR", "VARCHAR(100)", 100, null, null],
            ["LOCATION", "VARCHAR", "VARCHAR(200)", 200, null, null],
        ])
    );
    for column in result.as_array().unwrap() {
        assert_eq!(column["is_pk"], json!(false), "a view has no key");
    }
    // An outer-joined column is nullable even though its source is not.
    assert_eq!(result[6]["name"], json!("DEPT_NAME"));
    assert_eq!(result[6]["is_nullable"], json!(true));

    // They are the columns a query on the view returns.
    let rows = p.query_ok("SELECT * FROM TEST_SCHEMA.V_EMP_DETAILS", json!(1), 1);
    let names: Vec<&Value> = result
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["name"])
        .collect();
    assert_eq!(json!(names), rows["columns"]);

    // Computed columns: the types Db2 derived for them.
    let summary = p.call_ok(
        "get_view_columns",
        p.schema_params(json!({ "view_name": "V_DEPT_SUMMARY" })),
    );
    let names: Vec<Value> = summary
        .as_array()
        .unwrap()
        .iter()
        .map(|column| json!([column["name"], column["data_type"]]))
        .collect();
    assert_eq!(
        json!(names),
        json!([
            ["DEPT_ID", "INTEGER"],
            ["DEPT_NAME", "VARCHAR"],
            ["EMP_COUNT", "INTEGER"],
            ["AVG_SALARY", "DECIMAL"]
        ])
    );

    let missing = p.call_ok(
        "get_view_columns",
        p.schema_params(json!({ "view_name": "NOPE" })),
    );
    assert_eq!(missing, json!([]));
}

#[test]
fn test_create_alter_drop_view() {
    require_db2!();
    let mut p = Plugin::spawn();
    for view in ["V_TMP", "V_TMP_LIST", "V_TMP_LONG"] {
        p.query(&format!("DROP VIEW TEST_SCHEMA.{view}"), Value::Null, 1);
    }
    let view_names = |p: &mut Plugin| -> Vec<String> {
        p.call_ok("get_views", p.schema_params(json!({})))
            .as_array()
            .unwrap()
            .iter()
            .map(|view| view["name"].as_str().unwrap().to_string())
            .collect()
    };
    let definition_of = |p: &mut Plugin, view: &str| -> String {
        p.call_ok(
            "get_view_definition",
            p.schema_params(json!({ "view_name": view })),
        )
        .as_str()
        .unwrap()
        .to_string()
    };

    // Create from a query, as Tabularis's "Create New View" does.
    let created = p.call_ok(
        "create_view",
        p.schema_params(json!({
            "view_name": "V_TMP",
            "definition": "SELECT EMP_ID, LAST_NAME FROM EMPLOYEES WHERE DEPT_ID = 1;\n"
        })),
    );
    assert_eq!(created, Value::Null);
    assert!(view_names(&mut p).contains(&"V_TMP".to_string()));
    assert_eq!(
        definition_of(&mut p, "V_TMP"),
        "SELECT EMP_ID, LAST_NAME FROM EMPLOYEES WHERE DEPT_ID = 1"
    );
    let rows = p.query_ok(
        "SELECT * FROM TEST_SCHEMA.V_TMP ORDER BY EMP_ID",
        Value::Null,
        1,
    );
    assert_eq!(
        rows["rows"],
        json!([[1, "Doe"], [2, "Smith"], [4, "Williams"]])
    );

    // It already exists.
    let resp = p.call(
        "create_view",
        p.schema_params(
            json!({ "view_name": "V_TMP", "definition": "SELECT 1 AS X FROM SYSIBM.SYSDUMMY1" }),
        ),
    );
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("SQL0601N"),
        "{resp}"
    );

    // Alter: what get_view_definition returned, edited and sent back.
    let edited = definition_of(&mut p, "V_TMP").replace("DEPT_ID = 1", "DEPT_ID = 2");
    let altered = p.call_ok(
        "alter_view",
        p.schema_params(json!({ "view_name": "V_TMP", "definition": edited })),
    );
    assert_eq!(altered, Value::Null);
    let rows = p.query_ok("SELECT * FROM TEST_SCHEMA.V_TMP", Value::Null, 1);
    assert_eq!(rows["rows"], json!([[3, "Johnson"]]));
    assert_eq!(
        definition_of(&mut p, "V_TMP"),
        "SELECT EMP_ID, LAST_NAME FROM EMPLOYEES WHERE DEPT_ID = 2"
    );

    // A view with an explicit column list keeps it when its query is edited.
    p.query_ok(
        "CREATE VIEW TEST_SCHEMA.V_TMP_LIST (\"Who\", HOW_MUCH) AS SELECT LAST_NAME, SALARY FROM TEST_SCHEMA.EMPLOYEES",
        Value::Null,
        1,
    );
    assert_eq!(
        definition_of(&mut p, "V_TMP_LIST"),
        "SELECT LAST_NAME, SALARY FROM TEST_SCHEMA.EMPLOYEES"
    );
    p.call_ok(
        "alter_view",
        p.schema_params(json!({
            "view_name": "V_TMP_LIST",
            "definition": "SELECT FIRST_NAME, SALARY FROM TEST_SCHEMA.EMPLOYEES WHERE EMP_ID = 1"
        })),
    );
    let rows = p.query_ok("SELECT * FROM TEST_SCHEMA.V_TMP_LIST", Value::Null, 1);
    assert_eq!(
        rows["columns"],
        json!(["Who", "HOW_MUCH"]),
        "column names kept"
    );
    assert_eq!(rows["rows"], json!([["John", "75000.00"]]));

    // A definition far longer than one fetch buffer comes back whole.
    let branches: String = (0..600)
        .map(|n| format!("WHEN EMP_ID = {n} THEN 'employee number {n}' "))
        .collect();
    let long_query = format!(
        "SELECT EMP_ID, CASE {branches}ELSE 'other' END AS LABEL FROM TEST_SCHEMA.EMPLOYEES"
    );
    assert!(long_query.len() > 20_000);
    p.call_ok(
        "create_view",
        p.schema_params(json!({ "view_name": "V_TMP_LONG", "definition": long_query })),
    );
    assert_eq!(definition_of(&mut p, "V_TMP_LONG"), long_query);
    let listed = p.call_ok("get_views", p.schema_params(json!({})));
    let long = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["name"] == "V_TMP_LONG")
        .expect("V_TMP_LONG listed");
    assert_eq!(long["definition"].as_str().unwrap(), long_query);

    // A complete CREATE VIEW statement is accepted as a definition too.
    p.call_ok(
        "alter_view",
        p.schema_params(json!({
            "view_name": "V_TMP",
            "definition": "CREATE OR REPLACE VIEW TEST_SCHEMA.V_TMP AS SELECT EMP_ID FROM TEST_SCHEMA.EMPLOYEES WHERE EMP_ID = 6"
        })),
    );
    let rows = p.query_ok("SELECT * FROM TEST_SCHEMA.V_TMP", Value::Null, 1);
    assert_eq!(rows["rows"], json!([[6]]));

    // Errors: a query that does not compile, a view that is not there, no
    // query at all.
    let resp = p.call(
        "alter_view",
        p.schema_params(
            json!({ "view_name": "V_TMP", "definition": "SELECT NOPE FROM EMPLOYEES" }),
        ),
    );
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("SQL0206N"),
        "{resp}"
    );
    let rows = p.query_ok("SELECT * FROM TEST_SCHEMA.V_TMP", Value::Null, 1);
    assert_eq!(
        rows["rows"],
        json!([[6]]),
        "the view is unchanged after a failed alter"
    );
    let resp = p.call(
        "alter_view",
        p.schema_params(
            json!({ "view_name": "NOPE", "definition": "SELECT 1 AS X FROM SYSIBM.SYSDUMMY1" }),
        ),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("View \"TEST_SCHEMA\".\"NOPE\" was not found")
    );
    let resp = p.call(
        "create_view",
        p.schema_params(json!({ "view_name": "V_EMPTY", "definition": " ; " })),
    );
    assert_eq!(resp["error"]["message"], json!("A view needs a query"));

    // Drop.
    for view in ["V_TMP", "V_TMP_LIST", "V_TMP_LONG"] {
        let dropped = p.call_ok("drop_view", p.schema_params(json!({ "view_name": view })));
        assert_eq!(dropped, Value::Null);
    }
    assert_eq!(view_names(&mut p), ["V_DEPT_SUMMARY", "V_EMP_DETAILS"]);
    let resp = p.call(
        "drop_view",
        p.schema_params(json!({ "view_name": "V_TMP" })),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("SQL0204N \"TEST_SCHEMA.V_TMP\" is an undefined name. SQLSTATE=42704")
    );

    // Delimited names all the way.
    p.query("DROP VIEW \"Mixed Case\".\"Tmp View\"", Value::Null, 1);
    p.call_ok(
        "create_view",
        p.params_in(
            "Mixed Case",
            json!({ "view_name": "Tmp View", "definition": "SELECT \"O'Brien\" AS \"Who\" FROM \"Order Details\"" }),
        ),
    );
    let rows = p.query_ok("SELECT * FROM \"Mixed Case\".\"Tmp View\"", Value::Null, 1);
    assert_eq!(rows["rows"], json!([["it's"]]));
    p.call_ok(
        "drop_view",
        p.params_in("Mixed Case", json!({ "view_name": "Tmp View" })),
    );
}

// ---------------------------------------------------------------------------
// Tests — Foreign keys & Indexes
// ---------------------------------------------------------------------------

/// `[name, column, ref table, ref column, on delete, on update, position]`
/// of each foreign key entry.
fn foreign_key_entries(keys: &Value) -> Vec<Value> {
    keys.as_array()
        .expect("expected array")
        .iter()
        .map(|key| {
            json!([
                key["name"],
                key["column_name"],
                key["ref_table"],
                key["ref_column"],
                key["on_delete"],
                key["on_update"],
                key["seq_in_fk"]
            ])
        })
        .collect()
}

fn foreign_keys_of(p: &mut Plugin, table: &str) -> Vec<Value> {
    let keys = p.call_ok(
        "get_foreign_keys",
        p.schema_params(json!({ "table": table })),
    );
    foreign_key_entries(&keys)
}

#[test]
fn test_get_foreign_keys() {
    require_db2!();
    let mut p = Plugin::spawn();

    // One foreign key per rule Db2 has for deletes, spelled out.
    assert_eq!(
        foreign_keys_of(&mut p, "EMPLOYEES"),
        [json!([
            "FK_EMP_DEPT",
            "DEPT_ID",
            "DEPARTMENTS",
            "DEPT_ID",
            "SET NULL",
            "NO ACTION",
            1
        ])]
    );
    assert_eq!(
        foreign_keys_of(&mut p, "PROJECTS"),
        [json!([
            "FK_PROJ_DEPT",
            "DEPT_ID",
            "DEPARTMENTS",
            "DEPT_ID",
            "CASCADE",
            "NO ACTION",
            1
        ])]
    );
    assert_eq!(
        foreign_keys_of(&mut p, "FK_CHILD"),
        [json!([
            "FK_CHILD_PARENT",
            "PARENT_ID",
            "FK_PARENT",
            "ID",
            "RESTRICT",
            "NO ACTION",
            1
        ])]
    );
    assert_eq!(
        foreign_keys_of(&mut p, "DROP_TEST"),
        [json!([
            "FK_DROP_TEST",
            "PARENT_ID",
            "DROP_TEST_PARENT",
            "ID",
            "NO ACTION",
            "NO ACTION",
            1
        ])]
    );

    // Two foreign keys on one table, sorted by name.
    assert_eq!(
        foreign_keys_of(&mut p, "EMP_PROJECTS"),
        [
            json!([
                "FK_EP_EMP",
                "EMP_ID",
                "EMPLOYEES",
                "EMP_ID",
                "CASCADE",
                "NO ACTION",
                1
            ]),
            json!([
                "FK_EP_PROJ",
                "PROJECT_ID",
                "PROJECTS",
                "PROJECT_ID",
                "CASCADE",
                "NO ACTION",
                1
            ]),
        ]
    );

    // A composite foreign key: one entry per column, each paired with the
    // parent column at the same position, plus the rule on updates.
    assert_eq!(
        foreign_keys_of(&mut p, "COMPOSITE_CHILD"),
        [
            json!([
                "FK_COMPOSITE",
                "PARENT_REGION",
                "COMPOSITE_PARENT",
                "REGION",
                "CASCADE",
                "RESTRICT",
                1
            ]),
            json!([
                "FK_COMPOSITE",
                "PARENT_CODE",
                "COMPOSITE_PARENT",
                "CODE",
                "CASCADE",
                "RESTRICT",
                2
            ]),
        ]
    );

    // A table that is only referenced, one without any, one that does not
    // exist, and a view.
    for table in ["DEPARTMENTS", "KEYLESS_TEST", "NOPE", "V_EMP_DETAILS"] {
        assert_eq!(
            foreign_keys_of(&mut p, table),
            Vec::<Value>::new(),
            "{table}"
        );
    }
    let resp = p.call("get_foreign_keys", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");
}

#[test]
fn test_get_all_foreign_keys_batch() {
    require_db2!();
    let mut p = Plugin::spawn();

    let batch = p.call_ok("get_all_foreign_keys_batch", p.schema_params(json!({})));
    let map = batch.as_object().expect("expected object");

    // Exactly the tables that have foreign keys.
    let mut tables: Vec<&String> = map.keys().collect();
    tables.sort();
    assert_eq!(
        tables,
        [
            "COMPOSITE_CHILD",
            "DROP_TEST",
            "EMPLOYEES",
            "EMP_PROJECTS",
            "FK_CHILD",
            "PROJECTS"
        ]
    );

    // For each of them, what get_foreign_keys returns.
    for table in &tables {
        let single = p.call_ok(
            "get_foreign_keys",
            p.schema_params(json!({ "table": table })),
        );
        assert_eq!(map[table.as_str()], single, "{table}");
    }
    assert_eq!(
        foreign_key_entries(&map["COMPOSITE_CHILD"]),
        [
            json!([
                "FK_COMPOSITE",
                "PARENT_REGION",
                "COMPOSITE_PARENT",
                "REGION",
                "CASCADE",
                "RESTRICT",
                1
            ]),
            json!([
                "FK_COMPOSITE",
                "PARENT_CODE",
                "COMPOSITE_PARENT",
                "CODE",
                "CASCADE",
                "RESTRICT",
                2
            ]),
        ]
    );

    // `tables` narrows the batch; a table without foreign keys has no entry.
    let batch = p.call_ok(
        "get_all_foreign_keys_batch",
        p.schema_params(json!({ "tables": ["EMPLOYEES", "DEPARTMENTS", "NOPE"] })),
    );
    let keys: Vec<&String> = batch.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["EMPLOYEES"]);

    // A schema without foreign keys.
    for schema in ["EMPTY_SCHEMA", "Mixed Case"] {
        let batch = p.call_ok("get_all_foreign_keys_batch", p.params_in(schema, json!({})));
        assert_eq!(batch, json!({}), "{schema}");
    }

    // The schema snapshot carries the same foreign keys.
    let snapshot = p.call_ok("get_schema_snapshot", p.schema_params(json!({})));
    for table in snapshot.as_array().unwrap() {
        let name = table["name"].as_str().unwrap();
        let expected = map.get(name).cloned().unwrap_or_else(|| json!([]));
        assert_eq!(table["foreign_keys"], expected, "{name}");
    }
}

/// `[name, column, unique, primary, position, expression]` of each index
/// entry of a table in TEST_SCHEMA.
fn index_entries(p: &mut Plugin, table: &str) -> Vec<Value> {
    p.call_ok("get_indexes", p.schema_params(json!({ "table": table })))
        .as_array()
        .expect("expected array")
        .iter()
        .map(|index| {
            json!([
                index["name"],
                index["column_name"],
                index["is_unique"],
                index["is_primary"],
                index["seq_in_index"],
                index["is_expression"]
            ])
        })
        .collect()
}

#[test]
fn test_get_indexes() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Unique, non-unique and the primary key's index, each with its column.
    let entries = index_entries(&mut p, "EMPLOYEES");
    assert_eq!(entries.len(), 3, "{entries:?}");
    assert_eq!(
        entries[0],
        json!(["IDX_EMP_EMAIL", "EMAIL", true, false, 1, false])
    );
    assert_eq!(
        entries[1],
        json!(["IDX_EMP_LAST_NAME", "LAST_NAME", false, false, 1, false])
    );
    // Db2 names the primary key's index itself (SQL<timestamp>).
    assert!(
        entries[2][0].as_str().unwrap().starts_with("SQL"),
        "{entries:?}"
    );
    assert_eq!(
        entries[2].as_array().unwrap()[1..],
        json!(["EMP_ID", true, true, 1, false]).as_array().unwrap()[..]
    );

    // A composite index: one entry per column, in key order.
    let entries = index_entries(&mut p, "PROJECTS");
    assert_eq!(
        entries[0],
        json!(["IDX_PROJ_STATUS", "STATUS", false, false, 1, false])
    );
    assert_eq!(
        entries[1],
        json!(["IDX_PROJ_STATUS", "START_DATE", false, false, 2, false])
    );

    // A composite primary key.
    let entries = index_entries(&mut p, "EMP_PROJECTS");
    let primary: Vec<&Value> = entries
        .iter()
        .filter(|entry| entry[3] == json!(true))
        .collect();
    assert_eq!(primary.len(), 2);
    assert_eq!(primary[0][1], json!("EMP_ID"));
    assert_eq!(primary[0][4], json!(1));
    assert_eq!(primary[1][1], json!("PROJECT_ID"));
    assert_eq!(primary[1][4], json!(2));

    // A table without indexes, a table that does not exist, and a view.
    assert_eq!(index_entries(&mut p, "KEYLESS_TEST"), Vec::<Value>::new());
    assert_eq!(index_entries(&mut p, "NOPE"), Vec::<Value>::new());
    assert_eq!(index_entries(&mut p, "V_EMP_DETAILS"), Vec::<Value>::new());
    let resp = p.call("get_indexes", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");

    // Delimited names.
    let mixed = p.call_ok(
        "get_indexes",
        p.params_in("Mixed Case", json!({ "table": "Order Details" })),
    );
    assert_eq!(mixed.as_array().unwrap().len(), 1);
    assert_eq!(mixed[0]["column_name"], json!("Order Id"));
    assert_eq!(mixed[0]["is_primary"], json!(true));
}

#[test]
fn test_get_indexes_special_kinds() {
    require_db2!();
    let mut p = Plugin::spawn();
    for index in [
        "TEST_SCHEMA.IDX_T_EXPR",
        "TEST_SCHEMA.IDX_T_INCL",
        "\"Mixed Case\".\"Idx Elsewhere\"",
    ] {
        p.query(&format!("DROP INDEX {index}"), Value::Null, 1);
    }
    // An index on an expression with a descending column, a unique index
    // that carries an extra column, and an index kept in another schema.
    p.query_ok(
        "CREATE INDEX TEST_SCHEMA.IDX_T_EXPR ON TEST_SCHEMA.DEPARTMENTS (UPPER(DEPT_NAME), BUDGET DESC)",
        Value::Null,
        1,
    );
    p.query_ok(
        "CREATE UNIQUE INDEX TEST_SCHEMA.IDX_T_INCL ON TEST_SCHEMA.DEPARTMENTS (DEPT_NAME) INCLUDE (LOCATION)",
        Value::Null,
        1,
    );
    p.query_ok(
        "CREATE INDEX \"Mixed Case\".\"Idx Elsewhere\" ON TEST_SCHEMA.DEPARTMENTS (CREATED_AT)",
        Value::Null,
        1,
    );

    let entries = index_entries(&mut p, "DEPARTMENTS");
    let named = |name: &str| -> Vec<&Value> {
        entries
            .iter()
            .filter(|entry| entry[0] == json!(name))
            .collect()
    };
    // The expression is reported, not the name Db2 generates for it (K00).
    assert_eq!(
        named("IDX_T_EXPR"),
        [
            &json!(["IDX_T_EXPR", "UPPER(DEPT_NAME)", false, false, 1, true]),
            &json!(["IDX_T_EXPR", "BUDGET", false, false, 2, false]),
        ]
    );
    // The INCLUDE column is carried by the index but is not part of its key.
    assert_eq!(
        named("IDX_T_INCL"),
        [&json!(["IDX_T_INCL", "DEPT_NAME", true, false, 1, false])]
    );
    assert_eq!(
        named("Idx Elsewhere"),
        [&json!([
            "Idx Elsewhere",
            "CREATED_AT",
            false,
            false,
            1,
            false
        ])]
    );

    // drop_index finds each one, whichever schema it lives in.
    for index in ["IDX_T_EXPR", "IDX_T_INCL", "Idx Elsewhere"] {
        let dropped = p.call_ok(
            "drop_index",
            p.schema_params(json!({ "table": "DEPARTMENTS", "index_name": index })),
        );
        assert_eq!(dropped, Value::Null, "{index}");
    }
    let left = index_entries(&mut p, "DEPARTMENTS");
    assert_eq!(
        left.len(),
        1,
        "only the primary key's index is left: {left:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests — Routines
// ---------------------------------------------------------------------------

/// `NAME(signature)` of every routine `get_routines` lists, with its type.
fn routine_entries(plugin: &mut Plugin, schema: &str) -> Vec<(String, String)> {
    plugin
        .call_ok("get_routines", plugin.params_in(schema, json!({})))
        .as_array()
        .expect("expected array")
        .iter()
        .map(|routine| {
            (
                format!(
                    "{}({})",
                    routine["name"].as_str().expect("name"),
                    routine["identity_args"].as_str().expect("identity_args")
                ),
                routine["routine_type"]
                    .as_str()
                    .expect("routine_type")
                    .to_string(),
            )
        })
        .collect()
}

/// `MODE NAME TYPE @POSITION` of every parameter of a routine.
fn parameter_summary(plugin: &mut Plugin, schema: &str, target: Value) -> Vec<String> {
    plugin
        .call_ok("get_routine_parameters", plugin.params_in(schema, target))
        .as_array()
        .expect("expected array")
        .iter()
        .map(|parameter| {
            format!(
                "{} {} {} @{}",
                parameter["mode"].as_str().expect("mode"),
                parameter["name"].as_str().expect("name"),
                parameter["data_type"].as_str().expect("data_type"),
                parameter["ordinal_position"]
            )
        })
        .collect()
}

#[test]
fn test_get_routines() {
    require_db2!();
    let mut p = Plugin::spawn();

    let routines = routine_entries(&mut p, "TEST_SCHEMA");
    let expected = [
        (
            "ADJUST_BUDGET(INTEGER, DECIMAL(15,2), DECIMAL(15,2))",
            "PROCEDURE",
        ),
        ("DEPT_EMPLOYEE_COUNT(INTEGER)", "FUNCTION"),
        ("EMPLOYEES_OF_DEPT(INTEGER)", "FUNCTION"),
        ("FIXTURE_VERSION()", "FUNCTION"),
        ("FORMAT_AMOUNT(DECIMAL(10,2))", "FUNCTION"),
        ("FORMAT_AMOUNT(DECIMAL(10,2), CHAR(3))", "FUNCTION"),
        ("FULL_NAME(VARCHAR(50), VARCHAR(50))", "FUNCTION"),
        ("GET_EMPLOYEES_BY_DEPT(INTEGER)", "PROCEDURE"),
        (
            "UPDATE_SALARY(INTEGER, DECIMAL(10,2), DECIMAL(10,2))",
            "PROCEDURE",
        ),
    ];
    for (label, routine_type) in expected {
        assert!(
            routines.contains(&(label.to_string(), routine_type.to_string())),
            "{routine_type} {label} missing: {routines:?}"
        );
    }
}

#[test]
fn test_get_routines_reports_types_tabularis_groups_by() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The sidebar files a routine under Functions or Procedures by these two
    // words; the catalog's own `F` and `P` would leave both groups empty.
    for schema in ["TEST_SCHEMA", "SYSPROC", "SYSFUN"] {
        let routines = routine_entries(&mut p, schema);
        assert!(!routines.is_empty(), "{schema} has routines");
        for (label, routine_type) in &routines {
            assert!(
                routine_type == "FUNCTION" || routine_type == "PROCEDURE",
                "{schema}.{label} is a {routine_type}"
            );
        }
    }
}

#[test]
fn test_get_routines_leaves_definitions_out() {
    require_db2!();
    let mut p = Plugin::spawn();

    let result = p.call_ok("get_routines", p.schema_params(json!({})));
    for routine in result.as_array().expect("expected array") {
        assert!(routine["definition"].is_null(), "{routine}");
    }
}

#[test]
fn test_get_routines_empty_schema() {
    require_db2!();
    let mut p = Plugin::spawn();

    assert!(routine_entries(&mut p, "EMPTY_SCHEMA").is_empty());
}

#[test]
fn test_get_routine_parameters_directions_and_types() {
    require_db2!();
    let mut p = Plugin::spawn();

    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({
                "routine_name": "ADJUST_BUDGET",
                "identity_args": "INTEGER, DECIMAL(15,2), DECIMAL(15,2)"
            })
        ),
        [
            "IN P_DEPT_ID INTEGER @1",
            "OUT P_PREVIOUS DECIMAL(15,2) @2",
            "INOUT P_AMOUNT DECIMAL(15,2) @3",
        ]
    );
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "UPDATE_SALARY" })
        ),
        [
            "IN P_EMP_ID INTEGER @1",
            "IN P_NEW_SALARY DECIMAL(10,2) @2",
            "OUT P_OLD_SALARY DECIMAL(10,2) @3",
        ]
    );
}

#[test]
fn test_get_routine_parameters_function_return_value() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The value a function returns comes first, without a name: that is how
    // Tabularis recognises it and labels it "Return value".
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "FULL_NAME", "identity_args": "VARCHAR(50), VARCHAR(50)" })
        ),
        [
            "OUT  VARCHAR(101) @0",
            "IN P_FIRST_NAME VARCHAR(50) @1",
            "IN P_LAST_NAME VARCHAR(50) @2",
        ]
    );
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "FIXTURE_VERSION", "identity_args": "" })
        ),
        ["OUT  VARCHAR(10) @0"]
    );
}

#[test]
fn test_get_routine_parameters_table_function() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The columns of the returned table follow the arguments.
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "EMPLOYEES_OF_DEPT", "identity_args": "INTEGER" })
        ),
        [
            "IN P_DEPT_ID INTEGER @1",
            "OUT EMP_ID INTEGER @2",
            "OUT LAST_NAME VARCHAR(50) @3",
        ]
    );
}

#[test]
fn test_get_routine_parameters_of_each_overload() {
    require_db2!();
    let mut p = Plugin::spawn();

    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "FORMAT_AMOUNT", "identity_args": "DECIMAL(10,2)" })
        ),
        ["OUT  VARCHAR(20) @0", "IN P_AMOUNT DECIMAL(10,2) @1"]
    );
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "FORMAT_AMOUNT", "identity_args": "DECIMAL(10,2), CHAR(3)" })
        ),
        [
            "OUT  VARCHAR(24) @0",
            "IN P_AMOUNT DECIMAL(10,2) @1",
            "IN P_CURRENCY CHAR(3) @2",
        ]
    );
    // Without a signature the first routine of that name answers: never a
    // mix of the two.
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine_name": "FORMAT_AMOUNT" })
        ),
        ["OUT  VARCHAR(20) @0", "IN P_AMOUNT DECIMAL(10,2) @1"]
    );
}

#[test]
fn test_get_routine_parameters_accepts_guide_parameter_name() {
    require_db2!();
    let mut p = Plugin::spawn();

    // The plugin guide documents `routine`; Tabularis sends `routine_name`.
    assert_eq!(
        parameter_summary(
            &mut p,
            "TEST_SCHEMA",
            json!({ "routine": "DEPT_EMPLOYEE_COUNT" })
        ),
        ["OUT  INTEGER @0", "IN P_DEPT_ID INTEGER @1"]
    );
}

#[test]
fn test_get_routine_parameters_of_system_routines() {
    require_db2!();
    let mut p = Plugin::spawn();

    // An external procedure Db2 ships, with parameters of both directions.
    assert_eq!(
        parameter_summary(
            &mut p,
            "SYSPROC",
            json!({ "routine_name": "ADMIN_DROP_SCHEMA" })
        ),
        [
            "IN SCHEMA VARCHAR(128) @1",
            "IN DROPMODE VARCHAR(128) @2",
            "INOUT ERRORTABSCHEMA VARCHAR(128) @3",
            "INOUT ERRORTAB VARCHAR(128) @4",
        ]
    );
    // A built-in function: its argument has no name in the catalog, and is
    // shown by position rather than passed off as the return value.
    assert_eq!(
        parameter_summary(
            &mut p,
            "SYSIBM",
            json!({ "routine_name": "ABS", "identity_args": "DECFLOAT(34)" })
        ),
        ["OUT  DECFLOAT(34) @0", "IN $1 DECFLOAT(34) @1"]
    );
}

#[test]
fn test_get_routine_parameters_unknown_routine() {
    require_db2!();
    let mut p = Plugin::spawn();

    let resp = p.call(
        "get_routine_parameters",
        p.schema_params(json!({ "routine_name": "NO_SUCH_ROUTINE" })),
    );
    assert_eq!(
        resp["error"]["message"],
        "Routine \"TEST_SCHEMA\".\"NO_SUCH_ROUTINE\" does not exist"
    );

    let resp = p.call(
        "get_routine_parameters",
        p.schema_params(json!({ "routine_name": "FORMAT_AMOUNT", "identity_args": "DATE" })),
    );
    assert_eq!(
        resp["error"]["message"],
        "Routine \"TEST_SCHEMA\".\"FORMAT_AMOUNT\"(DATE) does not exist"
    );

    let resp = p.call("get_routine_parameters", p.schema_params(json!({})));
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
}

#[test]
fn test_get_routine_definition() {
    require_db2!();
    let mut p = Plugin::spawn();

    let definition = p.call_ok(
        "get_routine_definition",
        p.schema_params(json!({
            "routine_name": "FULL_NAME",
            "routine_type": "FUNCTION",
            "identity_args": "VARCHAR(50), VARCHAR(50)"
        })),
    );
    let definition = definition.as_str().expect("expected string");
    assert!(
        definition.starts_with("CREATE FUNCTION TEST_SCHEMA.FULL_NAME("),
        "{definition}"
    );
    assert!(
        definition.ends_with("RETURN p_first_name || ' ' || p_last_name;\nEND"),
        "the whole statement, as it was written: {definition}"
    );

    let definition = p.call_ok(
        "get_routine_definition",
        p.schema_params(json!({
            "routine_name": "UPDATE_SALARY",
            "routine_type": "PROCEDURE",
            "identity_args": "INTEGER, DECIMAL(10,2), DECIMAL(10,2)"
        })),
    );
    let definition = definition.as_str().expect("expected string");
    assert!(
        definition.starts_with("CREATE PROCEDURE TEST_SCHEMA.UPDATE_SALARY("),
        "{definition}"
    );
    assert!(
        definition.contains("OUT p_old_salary DECIMAL(10, 2)"),
        "{definition}"
    );
}

#[test]
fn test_get_routine_definition_of_each_overload() {
    require_db2!();
    let mut p = Plugin::spawn();

    let mut definition_of = |identity_args: &str| {
        p.call_ok(
            "get_routine_definition",
            p.schema_params(json!({
                "routine_name": "FORMAT_AMOUNT",
                "routine_type": "FUNCTION",
                "identity_args": identity_args
            })),
        )
        .as_str()
        .expect("expected string")
        .to_string()
    };

    let one_argument = definition_of("DECIMAL(10,2)");
    assert!(
        one_argument.contains("RETURNS VARCHAR(20)"),
        "{one_argument}"
    );
    assert!(!one_argument.contains("p_currency"), "{one_argument}");

    let two_arguments = definition_of("DECIMAL(10,2), CHAR(3)");
    assert!(
        two_arguments.contains("RETURNS VARCHAR(24)"),
        "{two_arguments}"
    );
    assert!(
        two_arguments.contains("p_currency CHAR(3)"),
        "{two_arguments}"
    );
}

#[test]
fn test_get_routine_definition_is_complete() {
    require_db2!();
    let mut p = Plugin::spawn();

    // A body far longer than one fetch buffer comes back whole.
    let filler: String = (0..400)
        .map(|line| format!("    -- line {line:03} of a long routine body, kept as written\n"))
        .collect();
    let statement = format!(
        "CREATE OR REPLACE PROCEDURE TEST_SCHEMA.LONG_ROUTINE(OUT p_total INTEGER)\n\
         LANGUAGE SQL\nBEGIN\n{filler}    SET p_total = 400;\nEND"
    );
    assert!(statement.len() > 20_000);
    p.query_ok(&statement, Value::Null, 1);

    let definition = p.call_ok(
        "get_routine_definition",
        p.schema_params(json!({
            "routine_name": "LONG_ROUTINE",
            "routine_type": "PROCEDURE",
            "identity_args": "INTEGER"
        })),
    );
    p.query_ok("DROP PROCEDURE TEST_SCHEMA.LONG_ROUTINE", Value::Null, 1);

    assert_eq!(definition.as_str().expect("expected string"), statement);
}

#[test]
fn test_get_routine_definition_tells_function_from_procedure() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Db2 keeps functions and procedures apart: the two can share a name
    // and a signature, and `routine_type` is what says which one is meant.
    p.query_ok(
        "CREATE OR REPLACE FUNCTION TEST_SCHEMA.TWIN_ROUTINE(p_value INTEGER) \
         RETURNS INTEGER LANGUAGE SQL RETURN p_value + 1",
        Value::Null,
        1,
    );
    p.query_ok(
        "CREATE OR REPLACE PROCEDURE TEST_SCHEMA.TWIN_ROUTINE(INOUT p_value INTEGER) \
         LANGUAGE SQL BEGIN SET p_value = p_value + 1; END",
        Value::Null,
        1,
    );

    let mut definition_of = |routine_type: &str| {
        p.call_ok(
            "get_routine_definition",
            p.schema_params(json!({
                "routine_name": "TWIN_ROUTINE",
                "routine_type": routine_type,
                "identity_args": "INTEGER"
            })),
        )
        .as_str()
        .expect("expected string")
        .to_string()
    };
    let function = definition_of("FUNCTION");
    let procedure = definition_of("PROCEDURE");

    p.query_ok("DROP FUNCTION TEST_SCHEMA.TWIN_ROUTINE", Value::Null, 1);
    p.query_ok("DROP PROCEDURE TEST_SCHEMA.TWIN_ROUTINE", Value::Null, 1);

    assert!(
        function.starts_with("CREATE OR REPLACE FUNCTION"),
        "{function}"
    );
    assert!(
        procedure.starts_with("CREATE OR REPLACE PROCEDURE"),
        "{procedure}"
    );
}

#[test]
fn test_get_routine_definition_without_stored_source() {
    require_db2!();
    let mut p = Plugin::spawn();

    // An external routine is compiled code: there is no source to return,
    // and the answer says so instead of opening an empty editor.
    let resp = p.call(
        "get_routine_definition",
        p.params_in(
            "SYSPROC",
            json!({ "routine_name": "ADMIN_CMD", "routine_type": "PROCEDURE", "identity_args": "CLOB(2M)" }),
        ),
    );
    assert_eq!(
        resp["error"]["message"],
        "\"SYSPROC\".\"ADMIN_CMD\"(CLOB(2M)) is an external C routine: \
         Db2 keeps its compiled code, not its source"
    );

    let resp = p.call(
        "get_routine_definition",
        p.params_in(
            "SYSIBM",
            json!({ "routine_name": "ABS", "routine_type": "FUNCTION", "identity_args": "DECFLOAT(34)" }),
        ),
    );
    assert_eq!(
        resp["error"]["message"],
        "\"SYSIBM\".\"ABS\"(DECFLOAT(34)) is built into Db2: it has no definition"
    );

    let resp = p.call(
        "get_routine_definition",
        p.schema_params(json!({ "routine_name": "FULL_NAME", "routine_type": "PROCEDURE" })),
    );
    assert_eq!(
        resp["error"]["message"],
        "Procedure \"TEST_SCHEMA\".\"FULL_NAME\" does not exist"
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
// Tests — DDL generation: every generated statement is run on Db2
// ---------------------------------------------------------------------------

/// A column as Tabularis's table designer sends it.
fn column_def(name: &str, data_type: &str) -> Value {
    json!({
        "name": name, "data_type": data_type, "is_nullable": true, "is_pk": false,
        "is_auto_increment": false, "default_value": null
    })
}

fn with(mut column: Value, changes: Value) -> Value {
    for (key, value) in changes.as_object().unwrap() {
        column[key] = value.clone();
    }
    column
}

/// Asks the plugin for DDL and returns the statements.
fn generate(p: &mut Plugin, method: &str, args: Value) -> Vec<String> {
    let result = p.call_ok(method, p.schema_params(args));
    result
        .as_array()
        .unwrap_or_else(|| panic!("{method} should return statements: {result}"))
        .iter()
        .map(|statement| statement.as_str().unwrap().to_string())
        .collect()
}

/// Runs generated statements one by one, as Tabularis does.
fn run_all(p: &mut Plugin, statements: &[String]) {
    for statement in statements {
        p.query_ok(statement, Value::Null, 1);
    }
}

/// `[name, full type, nullable, default, identity]` of each column.
fn described(p: &mut Plugin, table: &str) -> Value {
    let columns = p.call_ok("get_columns", p.schema_params(json!({ "table": table })));
    json!(columns
        .as_array()
        .unwrap()
        .iter()
        .map(|column| json!([
            column["name"],
            column
                .get("full_type")
                .cloned()
                .unwrap_or(column["data_type"].clone()),
            column["is_nullable"],
            column["default_value"],
            column["is_auto_increment"],
        ]))
        .collect::<Vec<_>>())
}

const DDL_TABLE: &str = "Ddl Test";

fn drop_ddl_table(p: &mut Plugin) {
    p.query(
        &format!("DROP TABLE TEST_SCHEMA.\"{DDL_TABLE}\""),
        Value::Null,
        1,
    );
}

/// Creates the table the DDL tests alter, through the plugin's own DDL.
fn create_ddl_table(p: &mut Plugin) -> Vec<String> {
    drop_ddl_table(p);
    let statements = generate(
        p,
        "get_create_table_sql",
        json!({
            "table_name": DDL_TABLE,
            "columns": [
                with(column_def("id", "INTEGER"), json!({ "is_pk": true, "is_auto_increment": true, "is_nullable": false })),
                with(column_def("Unit Price", "DECIMAL(9,2)"), json!({ "is_nullable": false, "default_value": "0" })),
                with(column_def("select", "VARCHAR(20)"), json!({ "default_value": "'new'", "comment": "It's a reserved word" })),
                with(column_def("created", "TIMESTAMP"), json!({ "is_nullable": false, "default_value": "CURRENT TIMESTAMP" })),
                column_def("dept", "INTEGER"),
            ]
        }),
    );
    run_all(p, &statements);
    statements
}

#[test]
fn test_ddl_create_table() {
    require_db2!();
    let mut p = Plugin::spawn();

    let statements = create_ddl_table(&mut p);
    assert_eq!(
        statements,
        [
            "CREATE TABLE \"TEST_SCHEMA\".\"Ddl Test\" (\n  \
             \"id\" INTEGER NOT NULL GENERATED BY DEFAULT AS IDENTITY,\n  \
             \"Unit Price\" DECIMAL(9,2) NOT NULL DEFAULT 0,\n  \
             \"select\" VARCHAR(20) DEFAULT 'new',\n  \
             \"created\" TIMESTAMP NOT NULL DEFAULT CURRENT TIMESTAMP,\n  \
             \"dept\" INTEGER,\n  \
             PRIMARY KEY (\"id\")\n)",
            "COMMENT ON COLUMN \"TEST_SCHEMA\".\"Ddl Test\".\"select\" IS 'It''s a reserved word'",
        ]
    );

    // The table Db2 built is the one that was asked for.
    assert_eq!(
        described(&mut p, DDL_TABLE),
        json!([
            ["id", "INTEGER", false, null, true],
            ["Unit Price", "DECIMAL(9,2)", false, "0", false],
            ["select", "VARCHAR(20)", true, "'new'", false],
            ["created", "TIMESTAMP", false, "CURRENT TIMESTAMP", false],
            ["dept", "INTEGER", true, null, false],
        ])
    );
    let columns = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": DDL_TABLE })),
    );
    assert_eq!(columns[0]["is_pk"], json!(true));
    assert_eq!(columns[2]["comment"], json!("It's a reserved word"));

    // It works: the identity and the defaults fill a row.
    p.query_ok(
        "INSERT INTO \"Ddl Test\" (\"dept\") VALUES (1)",
        Value::Null,
        1,
    );
    let row = p.query_ok(
        "SELECT \"id\", \"Unit Price\", \"select\" FROM \"Ddl Test\"",
        Value::Null,
        1,
    );
    assert_eq!(row["rows"], json!([[1, "0.00", "new"]]));

    // Every type the manifest offers can be used in a CREATE TABLE.
    p.query("DROP TABLE TEST_SCHEMA.DDL_ALL_TYPES", Value::Null, 1);
    let manifest: Value =
        serde_json::from_str(include_str!("../.tabularium")).expect("manifest is valid JSON");
    let columns: Vec<Value> = manifest["data_types"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, data_type)| {
            let name = data_type["name"].as_str().unwrap();
            let sized = match data_type["default_length"].as_str() {
                Some(length) if data_type["requires_length"] == json!(true) => {
                    format!("{name}({length})")
                }
                _ if data_type["requires_precision"] == json!(true) => format!("{name}(10,2)"),
                _ => name.to_string(),
            };
            column_def(&format!("C{index}"), &sized)
        })
        .collect();
    assert!(columns.len() >= 20, "manifest lists the Db2 types");
    let statements = generate(
        &mut p,
        "get_create_table_sql",
        json!({ "table_name": "DDL_ALL_TYPES", "columns": columns }),
    );
    run_all(&mut p, &statements);
    let created = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": "DDL_ALL_TYPES" })),
    );
    assert_eq!(created.as_array().unwrap().len(), columns.len());
    p.query_ok("DROP TABLE TEST_SCHEMA.DDL_ALL_TYPES", Value::Null, 1);

    // A composite primary key.
    p.query("DROP TABLE TEST_SCHEMA.DDL_COMPOSITE", Value::Null, 1);
    let statements = generate(
        &mut p,
        "get_create_table_sql",
        json!({
            "table_name": "DDL_COMPOSITE",
            "columns": [
                with(column_def("A", "INTEGER"), json!({ "is_pk": true })),
                with(column_def("B", "VARCHAR(10)"), json!({ "is_pk": true })),
                column_def("C", "DATE"),
            ]
        }),
    );
    run_all(&mut p, &statements);
    let keys: Vec<Value> = p
        .call_ok(
            "get_columns",
            p.schema_params(json!({ "table": "DDL_COMPOSITE" })),
        )
        .as_array()
        .unwrap()
        .iter()
        .map(|column| json!([column["name"], column["is_pk"], column["is_nullable"]]))
        .collect();
    assert_eq!(
        json!(keys),
        json!([["A", true, false], ["B", true, false], ["C", false, true]])
    );
    p.query_ok("DROP TABLE TEST_SCHEMA.DDL_COMPOSITE", Value::Null, 1);

    // A table needs a column.
    let resp = p.call(
        "get_create_table_sql",
        p.schema_params(json!({ "table_name": "EMPTY", "columns": [] })),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("A table needs at least one column")
    );

    drop_ddl_table(&mut p);
}

#[test]
fn test_ddl_add_column() {
    require_db2!();
    let mut p = Plugin::spawn();
    create_ddl_table(&mut p);
    p.query_ok(
        "INSERT INTO \"Ddl Test\" (\"dept\") VALUES (1), (2)",
        Value::Null,
        1,
    );

    // A nullable column with a comment.
    let statements = generate(
        &mut p,
        "get_add_column_sql",
        json!({
            "table": DDL_TABLE,
            "column": with(column_def("Qty#", "SMALLINT"), json!({ "comment": "How many" }))
        }),
    );
    assert_eq!(
        statements,
        [
            "ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ADD COLUMN \"Qty#\" SMALLINT",
            "COMMENT ON COLUMN \"TEST_SCHEMA\".\"Ddl Test\".\"Qty#\" IS 'How many'",
        ]
    );
    run_all(&mut p, &statements);

    // NOT NULL on a table that already has rows: Db2 demands a default, so
    // one is generated (WITH DEFAULT) unless the user gave one.
    let statements = generate(
        &mut p,
        "get_add_column_sql",
        json!({ "table": DDL_TABLE, "column": with(column_def("required", "INTEGER"), json!({ "is_nullable": false })) }),
    );
    assert_eq!(
        statements,
        ["ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ADD COLUMN \"required\" INTEGER NOT NULL WITH DEFAULT"]
    );
    run_all(&mut p, &statements);

    let statements = generate(
        &mut p,
        "get_add_column_sql",
        json!({
            "table": DDL_TABLE,
            "column": with(column_def("flag", "BOOLEAN"), json!({ "is_nullable": false, "default_value": "TRUE" }))
        }),
    );
    run_all(&mut p, &statements);

    let columns = described(&mut p, DDL_TABLE);
    assert_eq!(columns[5], json!(["Qty#", "SMALLINT", true, null, false]));
    assert_eq!(
        columns[6],
        json!(["required", "INTEGER", false, "0", false])
    );
    assert_eq!(columns[7], json!(["flag", "BOOLEAN", false, "TRUE", false]));

    // The existing rows got the defaults and the table is usable right away.
    let rows = p.query_ok(
        "SELECT \"Qty#\", \"required\", \"flag\" FROM \"Ddl Test\" ORDER BY \"id\"",
        Value::Null,
        1,
    );
    assert_eq!(rows["rows"], json!([[null, 0, true], [null, 0, true]]));

    drop_ddl_table(&mut p);
}

#[test]
fn test_ddl_alter_column() {
    require_db2!();
    let mut p = Plugin::spawn();
    create_ddl_table(&mut p);
    p.query_ok(
        "INSERT INTO \"Ddl Test\" (\"dept\") VALUES (1), (2)",
        Value::Null,
        1,
    );

    let old_select = with(
        column_def("select", "VARCHAR(20)"),
        json!({ "default_value": "'new'", "comment": "It's a reserved word" }),
    );

    // Nothing changed: nothing to run.
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({ "table": DDL_TABLE, "old_column": old_select, "new_column": old_select }),
    );
    assert!(statements.is_empty(), "{statements:?}");

    // Rename, lengthen, make required, change the default and clear the
    // comment in one go.
    let new_status = with(
        column_def("Status", "VARCHAR(40)"),
        json!({ "is_nullable": false, "default_value": "'open'" }),
    );
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({ "table": DDL_TABLE, "old_column": old_select, "new_column": new_status }),
    );
    assert_eq!(
        statements,
        [
            "ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" RENAME COLUMN \"select\" TO \"Status\"",
            "ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ALTER COLUMN \"Status\" SET DATA TYPE VARCHAR(40)",
            "ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ALTER COLUMN \"Status\" SET DEFAULT 'open'",
            "ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ALTER COLUMN \"Status\" SET NOT NULL",
            "COMMENT ON COLUMN \"TEST_SCHEMA\".\"Ddl Test\".\"Status\" IS ''",
            "CALL SYSPROC.ADMIN_CMD('REORG TABLE \"TEST_SCHEMA\".\"Ddl Test\"')",
        ]
    );
    run_all(&mut p, &statements);
    assert_eq!(
        described(&mut p, DDL_TABLE)[2],
        json!(["Status", "VARCHAR(40)", false, "'open'", false])
    );
    let columns = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": DDL_TABLE })),
    );
    assert!(columns[2].get("comment").is_none(), "comment cleared");

    // A type change Db2 does not apply in place: without the generated
    // REORG the table would refuse every statement (SQL0668N).
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({
            "table": DDL_TABLE,
            "old_column": column_def("dept", "INTEGER"),
            "new_column": column_def("dept", "BIGINT")
        }),
    );
    assert_eq!(statements.len(), 2);
    p.query_ok(&statements[0], Value::Null, 1);
    let pending = p.query("SELECT COUNT(*) FROM \"Ddl Test\"", Value::Null, 1);
    let message = pending["error"]["message"].as_str().expect("reorg pending");
    assert!(message.starts_with("SQL0668N"), "{message}");
    assert!(
        message
            .ends_with("run CALL SYSPROC.ADMIN_CMD('REORG TABLE \"TEST_SCHEMA\".\"Ddl Test\"')."),
        "the error says how to fix it: {message}"
    );
    p.query_ok(&statements[1], Value::Null, 1);
    let rows = p.query_ok("SELECT COUNT(*) FROM \"Ddl Test\"", Value::Null, 1);
    assert_eq!(rows["rows"], json!([[2]]));
    assert_eq!(
        described(&mut p, DDL_TABLE)[4],
        json!(["dept", "BIGINT", true, null, false])
    );

    // Back to nullable, default dropped.
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({ "table": DDL_TABLE, "old_column": new_status, "new_column": column_def("Status", "VARCHAR(40)") }),
    );
    run_all(&mut p, &statements);
    assert_eq!(
        described(&mut p, DDL_TABLE)[2],
        json!(["Status", "VARCHAR(40)", true, null, false])
    );

    // Identity off and on again.
    let identity = with(
        column_def("id", "INTEGER"),
        json!({ "is_pk": true, "is_auto_increment": true, "is_nullable": false }),
    );
    let plain = with(
        column_def("id", "INTEGER"),
        json!({ "is_pk": true, "is_nullable": false }),
    );
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({ "table": DDL_TABLE, "old_column": identity, "new_column": plain }),
    );
    assert_eq!(
        statements,
        ["ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ALTER COLUMN \"id\" DROP IDENTITY"]
    );
    run_all(&mut p, &statements);
    assert_eq!(described(&mut p, DDL_TABLE)[0][4], json!(false));
    let statements = generate(
        &mut p,
        "get_alter_column_sql",
        json!({ "table": DDL_TABLE, "old_column": plain, "new_column": identity }),
    );
    run_all(&mut p, &statements);
    assert_eq!(described(&mut p, DDL_TABLE)[0][4], json!(true));

    // The data survived all of it.
    let rows = p.query_ok(
        "SELECT \"id\", \"Status\", \"dept\" FROM \"Ddl Test\" ORDER BY \"id\"",
        Value::Null,
        1,
    );
    assert_eq!(rows["rows"], json!([[1, "new", 1], [2, "new", 2]]));

    drop_ddl_table(&mut p);
}

#[test]
fn test_ddl_create_index() {
    require_db2!();
    let mut p = Plugin::spawn();
    create_ddl_table(&mut p);

    let statements = generate(
        &mut p,
        "get_create_index_sql",
        json!({ "table": DDL_TABLE, "index_name": "idx select", "columns": ["select", "Unit Price"], "is_unique": false }),
    );
    assert_eq!(
        statements,
        ["CREATE INDEX \"TEST_SCHEMA\".\"idx select\" ON \"TEST_SCHEMA\".\"Ddl Test\" (\"select\", \"Unit Price\")"]
    );
    run_all(&mut p, &statements);

    let statements = generate(
        &mut p,
        "get_create_index_sql",
        json!({ "table": DDL_TABLE, "index_name": "UX_DDL", "columns": ["created"], "is_unique": true }),
    );
    assert_eq!(
        statements,
        ["CREATE UNIQUE INDEX \"TEST_SCHEMA\".\"UX_DDL\" ON \"TEST_SCHEMA\".\"Ddl Test\" (\"created\")"]
    );
    run_all(&mut p, &statements);

    let indexes = p.call_ok(
        "get_indexes",
        p.schema_params(json!({ "table": DDL_TABLE })),
    );
    let listed: Vec<Value> = indexes
        .as_array()
        .unwrap()
        .iter()
        .filter(|index| index["is_primary"] == json!(false))
        .map(|index| {
            json!([
                index["name"],
                index["column_name"],
                index["is_unique"],
                index["seq_in_index"]
            ])
        })
        .collect();
    assert_eq!(
        json!(listed),
        json!([
            ["UX_DDL", "created", true, 1],
            ["idx select", "select", false, 1],
            ["idx select", "Unit Price", false, 2],
        ])
    );

    // The unique index is enforced.
    p.query_ok(
        "INSERT INTO \"Ddl Test\" (\"created\") VALUES ('2024-01-01-00.00.00')",
        Value::Null,
        1,
    );
    let duplicate = p.query(
        "INSERT INTO \"Ddl Test\" (\"created\") VALUES ('2024-01-01-00.00.00')",
        Value::Null,
        1,
    );
    assert!(duplicate["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("SQL0803N"));

    for (name, columns) in [("", json!(["select"])), ("IX", json!([]))] {
        let resp = p.call(
            "get_create_index_sql",
            p.schema_params(json!({ "table": DDL_TABLE, "index_name": name, "columns": columns, "is_unique": false })),
        );
        assert!(resp.get("error").is_some(), "{resp}");
    }

    drop_ddl_table(&mut p);
}

#[test]
fn test_ddl_create_foreign_key() {
    require_db2!();
    let mut p = Plugin::spawn();
    create_ddl_table(&mut p);

    let statements = generate(
        &mut p,
        "get_create_foreign_key_sql",
        json!({
            "table": DDL_TABLE, "fk_name": "FK ddl dept", "column": "dept",
            "ref_table": "DEPARTMENTS", "ref_column": "DEPT_ID",
            "on_delete": "SET NULL", "on_update": "NO ACTION"
        }),
    );
    assert_eq!(
        statements,
        ["ALTER TABLE \"TEST_SCHEMA\".\"Ddl Test\" ADD CONSTRAINT \"FK ddl dept\" FOREIGN KEY (\"dept\") \
          REFERENCES \"TEST_SCHEMA\".\"DEPARTMENTS\" (\"DEPT_ID\") ON DELETE SET NULL ON UPDATE NO ACTION"]
    );
    run_all(&mut p, &statements);

    let keys = p.call_ok(
        "get_foreign_keys",
        p.schema_params(json!({ "table": DDL_TABLE })),
    );
    assert_eq!(keys.as_array().unwrap().len(), 1);
    assert_eq!(keys[0]["name"], json!("FK ddl dept"));
    assert_eq!(keys[0]["column_name"], json!("dept"));
    assert_eq!(keys[0]["ref_table"], json!("DEPARTMENTS"));
    assert_eq!(keys[0]["ref_column"], json!("DEPT_ID"));

    // The constraint is enforced.
    let orphan = p.query(
        "INSERT INTO \"Ddl Test\" (\"dept\") VALUES (999)",
        Value::Null,
        1,
    );
    assert!(orphan["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("SQL0530N"));
    p.query_ok(
        "INSERT INTO \"Ddl Test\" (\"dept\") VALUES (1)",
        Value::Null,
        1,
    );

    // Each rule Db2 has for deletes produces a statement Db2 accepts.
    for (index, rule) in ["NO ACTION", "RESTRICT", "CASCADE"].iter().enumerate() {
        let name = format!("FK_RULE_{index}");
        let statements = generate(
            &mut p,
            "get_create_foreign_key_sql",
            json!({
                "table": DDL_TABLE, "fk_name": name, "column": "dept",
                "ref_table": "DEPARTMENTS", "ref_column": "DEPT_ID",
                "on_delete": rule, "on_update": "RESTRICT"
            }),
        );
        run_all(&mut p, &statements);
    }

    // Rules Db2 does not have are refused instead of generating a statement
    // that cannot run.
    let resp = p.call(
        "get_create_foreign_key_sql",
        p.schema_params(json!({
            "table": DDL_TABLE, "fk_name": "FK_BAD", "column": "dept",
            "ref_table": "DEPARTMENTS", "ref_column": "DEPT_ID",
            "on_delete": "CASCADE", "on_update": "CASCADE"
        })),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("Db2 does not support ON UPDATE CASCADE; use NO ACTION, RESTRICT")
    );

    drop_ddl_table(&mut p);
}

#[test]
fn test_get_table_ddl() {
    require_db2!();
    let mut p = Plugin::spawn();

    let ddl = p.call_ok(
        "get_table_ddl",
        p.schema_params(json!({ "table": "EMPLOYEES" })),
    );
    let ddl = ddl.as_str().expect("DDL text");
    assert!(
        ddl.starts_with(
            "CREATE TABLE \"TEST_SCHEMA\".\"EMPLOYEES\" (\n  \
             \"EMP_ID\" INTEGER NOT NULL GENERATED ALWAYS AS IDENTITY,\n  \
             \"FIRST_NAME\" VARCHAR(50) NOT NULL,\n"
        ),
        "{ddl}"
    );
    for expected in [
        "\"SALARY\" DECIMAL(10,2),\n",
        "\"IS_ACTIVE\" SMALLINT DEFAULT 1,\n",
        "\"NOTES\" CLOB(1M),\n",
        "PRIMARY KEY (\"EMP_ID\")\n);",
        "CREATE UNIQUE INDEX \"TEST_SCHEMA\".\"IDX_EMP_EMAIL\" ON \"TEST_SCHEMA\".\"EMPLOYEES\" (\"EMAIL\");",
        "CREATE INDEX \"TEST_SCHEMA\".\"IDX_EMP_LAST_NAME\" ON \"TEST_SCHEMA\".\"EMPLOYEES\" (\"LAST_NAME\");",
        "COMMENT ON TABLE \"TEST_SCHEMA\".\"EMPLOYEES\" IS 'Employee''s résumé\n所有员工';",
        "COMMENT ON COLUMN \"TEST_SCHEMA\".\"EMPLOYEES\".\"NOTES\" IS",
    ] {
        assert!(ddl.contains(expected), "missing {expected:?} in:\n{ddl}");
    }
    assert!(!ddl.contains("FOREIGN KEY"), "foreign keys are left out");

    // The DDL of the type-coverage table, run under another name, rebuilds
    // a table with the same columns.
    let ddl = p.call_ok(
        "get_table_ddl",
        p.schema_params(json!({ "table": "TYPE_METADATA_TEST" })),
    );
    let ddl = ddl.as_str().unwrap();
    assert!(
        ddl.contains("\"COL_COMPUTED\" INTEGER GENERATED ALWAYS AS (LENGTH(COL_VARCHAR))"),
        "{ddl}"
    );
    assert!(ddl.contains("\"COL_HIDDEN\" TIMESTAMP NOT NULL GENERATED ALWAYS FOR EACH ROW ON UPDATE AS ROW CHANGE TIMESTAMP IMPLICITLY HIDDEN"), "{ddl}");
    assert!(
        ddl.contains("\"COL_BIT_DATA\" VARCHAR(16) FOR BIT DATA"),
        "{ddl}"
    );
    assert!(ddl.contains("\"COL_TIMESTAMP12\" TIMESTAMP(12)"), "{ddl}");

    p.query("DROP TABLE TEST_SCHEMA.TYPE_METADATA_COPY", Value::Null, 1);
    let copy = ddl.replace("\"TYPE_METADATA_TEST\"", "\"TYPE_METADATA_COPY\"");
    for statement in copy.split(";\n\n") {
        p.query_ok(statement, Value::Null, 1);
    }
    let original = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": "TYPE_METADATA_TEST" })),
    );
    let rebuilt = p.call_ok(
        "get_columns",
        p.schema_params(json!({ "table": "TYPE_METADATA_COPY" })),
    );
    assert_eq!(rebuilt, original, "the rebuilt table has the same columns");
    p.query_ok("DROP TABLE TEST_SCHEMA.TYPE_METADATA_COPY", Value::Null, 1);

    // Delimited names, and a composite key.
    let ddl = p.call_ok(
        "get_table_ddl",
        p.params_in("Mixed Case", json!({ "table": "Order Details" })),
    );
    assert!(ddl.as_str().unwrap().starts_with(
        "CREATE TABLE \"Mixed Case\".\"Order Details\" (\n  \"Order Id\" INTEGER NOT NULL,\n  \
         \"Unit Price\" DECIMAL(9,2) NOT NULL DEFAULT 0,\n  \"select\" VARCHAR(20),"
    ));
    let ddl = p.call_ok(
        "get_table_ddl",
        p.schema_params(json!({ "table": "EMP_PROJECTS" })),
    );
    assert!(ddl
        .as_str()
        .unwrap()
        .contains("PRIMARY KEY (\"EMP_ID\", \"PROJECT_ID\")"));

    // Not a table.
    let resp = p.call(
        "get_table_ddl",
        p.schema_params(json!({ "table": "V_EMP_DETAILS" })),
    );
    assert_eq!(
        resp["error"]["message"],
        json!("\"TEST_SCHEMA\".\"V_EMP_DETAILS\" is a view, not a table")
    );
    let resp = p.call("get_table_ddl", p.schema_params(json!({ "table": "NOPE" })));
    assert_eq!(
        resp["error"]["message"],
        json!("Table \"TEST_SCHEMA\".\"NOPE\" was not found")
    );
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
            "pk_map": { "ID": 100 },
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
            "pk_map": { "ID": 100 }
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
// Tests — Insert record
// ---------------------------------------------------------------------------

/// Rows the insert tests add to TYPE_METADATA_TEST use ids from 1000 up, so
/// they can be removed without touching the fixture rows.
fn delete_inserted_type_rows(p: &mut Plugin) {
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.TYPE_METADATA_TEST WHERE ID >= 1000",
        Value::Null,
        1,
    );
}

/// One row of TYPE_METADATA_TEST as a `column -> value` map.
fn type_row(p: &mut Plugin, id: i64) -> serde_json::Map<String, Value> {
    let result = p.query_ok(
        &format!("SELECT * FROM TEST_SCHEMA.TYPE_METADATA_TEST WHERE ID = {id}"),
        json!(1),
        1,
    );
    let columns = result["columns"].as_array().unwrap();
    let row = result["rows"][0]
        .as_array()
        .unwrap_or_else(|| panic!("row {id} not found"));
    columns
        .iter()
        .zip(row)
        .map(|(column, value)| (column.as_str().unwrap().to_string(), value.clone()))
        .collect()
}

#[test]
fn test_insert_record_all_types() {
    require_db2!();
    let mut p = Plugin::spawn();
    delete_inserted_type_rows(&mut p);

    // Every value as a string, which is what the grid sends for a typed cell.
    let inserted = p.call_ok(
        "insert_record",
        p.schema_params(json!({
            "table": "TYPE_METADATA_TEST",
            "data": {
                "ID": "1000",
                "COL_CHAR": "xyz",
                "COL_VARCHAR": "O'Brien \"q\" 所有; DROP TABLE X; --",
                "COL_BIT_DATA": "CAFEBABE",
                "COL_GRAPHIC": "グラ",
                "COL_VARGRAPHIC": "变长",
                "COL_SMALLINT": "7",
                "COL_INTEGER": "-42",
                "COL_DECIMAL": "1234.5678",
                "COL_DECFLOAT16": "1.5",
                "COL_DECFLOAT34": "12345678901234567890.123456789",
                "COL_REAL": "1.25",
                "COL_DOUBLE": "75000.5",
                "COL_BOOLEAN": "true",
                "COL_DATE": "2024-02-29",
                "COL_TIME": "13:45:10",
                "COL_TIMESTAMP": "2024-02-29T13:45:10.123456",
                "COL_TIMESTAMP12": "2024-02-29 13:45:10.123456789012",
                "COL_CLOB": "clob",
                "COL_DBCLOB": "dbclob 文本",
                "COL_BLOB": "BLOB:4:application/octet-stream:yv66vg==",
                "COL_BINARY": "00112233445566778899AABBCCDDEEFF",
                "COL_VARBINARY": "DEADBEEF",
                "COL_XML": "<a><b>1</b></a>"
            }
        })),
    );
    assert_eq!(inserted, json!(1), "one row inserted");

    let row = type_row(&mut p, 1000);
    for (name, expected) in [
        ("ID", json!(1000)),
        ("COL_CHAR", json!("xyz       ")),
        ("COL_VARCHAR", json!("O'Brien \"q\" 所有; DROP TABLE X; --")),
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
        ("COL_CLOB", json!("clob")),
        ("COL_DBCLOB", json!("dbclob 文本")),
        (
            "COL_BLOB",
            json!("BLOB:4:application/octet-stream:yv66vg=="),
        ),
        ("COL_BINARY", json!("00112233445566778899AABBCCDDEEFF")),
        ("COL_VARBINARY", json!("DEADBEEF")),
        // Computed by Db2 from COL_VARCHAR.
        ("COL_COMPUTED", json!(36)),
    ] {
        assert_eq!(row[name], expected, "{name}");
    }
    assert!(row["COL_XML"]
        .as_str()
        .unwrap()
        .ends_with("<a><b>1</b></a>"));

    // The same row from native JSON values.
    p.call_ok(
        "insert_record",
        p.schema_params(json!({
            "table": "TYPE_METADATA_TEST",
            "data": {
                "ID": 1001, "COL_SMALLINT": 3, "COL_INTEGER": -1, "COL_DECIMAL": 12.5,
                "COL_DOUBLE": 1e3, "COL_BOOLEAN": false, "COL_VARCHAR": 12
            }
        })),
    );
    let row = type_row(&mut p, 1001);
    assert_eq!(row["COL_SMALLINT"], json!(3));
    assert_eq!(row["COL_DECIMAL"], json!("12.5000"));
    assert_eq!(row["COL_DOUBLE"], json!(1000.0));
    assert_eq!(row["COL_BOOLEAN"], json!(false));
    assert_eq!(row["COL_VARCHAR"], json!("12"));

    delete_inserted_type_rows(&mut p);
}

#[test]
fn test_insert_record_nulls_and_defaults() {
    require_db2!();
    let mut p = Plugin::spawn();
    delete_inserted_type_rows(&mut p);

    // NULL, "use the default", and a cleared numeric / date / boolean cell
    // (an empty string), which means "no value" rather than 0 or ''.
    p.call_ok(
        "insert_record",
        p.schema_params(json!({
            "table": "TYPE_METADATA_TEST",
            "data": {
                "ID": 1002,
                "COL_CHAR": "__USE_DEFAULT__",
                "COL_SMALLINT": "__USE_DEFAULT__",
                "COL_VARCHAR": null,
                "COL_INTEGER": "",
                "COL_DECIMAL": "  ",
                "COL_DATE": "",
                "COL_BOOLEAN": "",
                "COL_CLOB": "",
                "COL_BLOB": null
            }
        })),
    );
    let row = type_row(&mut p, 1002);
    assert_eq!(row["COL_CHAR"], json!("N/A       "), "column default");
    assert_eq!(row["COL_SMALLINT"], json!(1), "column default");
    for name in [
        "COL_VARCHAR",
        "COL_INTEGER",
        "COL_DECIMAL",
        "COL_DATE",
        "COL_BOOLEAN",
        "COL_BLOB",
    ] {
        assert_eq!(row[name], Value::Null, "{name}");
    }
    // For text, an empty string is an empty string.
    assert_eq!(row["COL_CLOB"], json!(""));
    // A column that was not mentioned gets its default.
    assert!(row["COL_TIMESTAMP"].as_str().is_some());

    // No values at all: a row of defaults (the identity generates the key).
    let before = p.query_ok(
        "SELECT MAX(ID) FROM TEST_SCHEMA.TYPE_METADATA_TEST",
        Value::Null,
        1,
    );
    let inserted = p.call_ok(
        "insert_record",
        p.schema_params(json!({ "table": "TYPE_METADATA_TEST", "data": {} })),
    );
    assert_eq!(inserted, json!(1));
    let after = p.query_ok(
        "SELECT ID, COL_CHAR, COL_BOOLEAN FROM TEST_SCHEMA.TYPE_METADATA_TEST ORDER BY COL_TIMESTAMP DESC",
        json!(1),
        1,
    );
    assert_eq!(after["rows"][0][1], json!("N/A       "));
    assert_eq!(after["rows"][0][2], json!(false));
    let generated = after["rows"][0][0].as_i64().unwrap();
    assert_ne!(Some(generated), before["rows"][0][0].as_i64());
    p.query_ok(
        &format!("DELETE FROM TEST_SCHEMA.TYPE_METADATA_TEST WHERE ID = {generated}"),
        Value::Null,
        1,
    );

    delete_inserted_type_rows(&mut p);
}

#[test]
fn test_insert_record_large_values() {
    require_db2!();
    let mut p = Plugin::spawn();
    delete_inserted_type_rows(&mut p);

    // A 280,000-byte CLOB: far beyond the 32,672 bytes a string constant
    // may hold, so it only works as a bound parameter.
    let clob = "€uro-".repeat(40_000);

    // A 1 MiB BLOB handed over as a file, the way Tabularis passes a picked
    // or uploaded file.
    let bytes: Vec<u8> = (0..1_048_576u32).map(|i| (i * 31 % 251) as u8).collect();
    let path = std::env::temp_dir().join(format!("db2-plugin-it-{}.bin", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();

    p.call_ok(
        "insert_record",
        p.schema_params(json!({
            "table": "TYPE_METADATA_TEST",
            "data": {
                "ID": 1003,
                "COL_CLOB": clob,
                "COL_BLOB": format!("BLOB_FILE_REF:{}:application/octet-stream:{}", bytes.len(), path.display())
            },
            "max_blob_size": 4 * 1024 * 1024
        })),
    );

    let stored = p.query_ok(
        "SELECT LENGTH(COL_CLOB), LENGTH(COL_BLOB), COL_CLOB, COL_BLOB FROM TEST_SCHEMA.TYPE_METADATA_TEST WHERE ID = 1003",
        json!(1),
        1,
    );
    let row = &stored["rows"][0];
    assert_eq!(row[0], json!(280_000), "bytes of text stored");
    assert_eq!(row[1], json!(1_048_576), "bytes stored");
    assert_eq!(row[2].as_str().unwrap(), clob);
    // In a result set a BLOB is a preview with its true size.
    assert!(row[3].as_str().unwrap().starts_with("BLOB:1048576:"));

    let key =
        json!({ "table": "TYPE_METADATA_TEST", "col_name": "COL_BLOB", "pk_map": { "ID": 1003 } });

    // fetch_blob_as_data_url returns every byte.
    let wire = p.call_ok("fetch_blob_as_data_url", p.schema_params(key.clone()));
    let wire = wire.as_str().unwrap();
    let prefix = "BLOB:1048576:application/octet-stream:";
    assert!(wire.starts_with(prefix), "{}", &wire[..60]);
    assert_eq!(wire.len() - prefix.len(), 1_048_576_usize.div_ceil(3) * 4);

    // save_blob_to_file writes the same bytes back to disk.
    let out = std::env::temp_dir().join(format!("db2-plugin-it-{}.out", std::process::id()));
    let mut save = key.clone();
    save["file_path"] = json!(out.display().to_string());
    let saved = p.call_ok("save_blob_to_file", p.schema_params(save));
    assert_eq!(saved, Value::Null);
    assert_eq!(std::fs::read(&out).unwrap(), bytes, "BLOB round trip");

    // A file larger than the host allows is refused before it is read.
    let resp = p.call(
        "insert_record",
        p.schema_params(json!({
            "table": "TYPE_METADATA_TEST",
            "data": {
                "ID": 1004,
                "COL_BLOB": format!("BLOB_FILE_REF:{}:application/octet-stream:{}", bytes.len(), path.display())
            },
            "max_blob_size": 1024
        })),
    );
    let message = resp["error"]["message"].as_str().expect("size error");
    assert!(
        message.contains("exceeds the maximum allowed BLOB size"),
        "{message}"
    );

    // A row that does not exist, a NULL value and a missing key are errors.
    let mut missing = key.clone();
    missing["pk_map"] = json!({ "ID": 424242 });
    let resp = p.call("fetch_blob_as_data_url", p.schema_params(missing));
    assert!(resp["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("No value found"));
    let mut keyless = key;
    keyless["pk_map"] = json!({});
    let resp = p.call("fetch_blob_as_data_url", p.schema_params(keyless));
    assert!(resp["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Cannot identify the row"));

    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(&out).unwrap();
    delete_inserted_type_rows(&mut p);
}

#[test]
fn test_insert_record_delimited_names() {
    require_db2!();
    let mut p = Plugin::spawn();
    let cleanup = "DELETE FROM \"Mixed Case\".\"Order Details\" WHERE \"Order Id\" = 2";
    p.query_ok(cleanup, Value::Null, 1);

    let inserted = p.call_ok(
        "insert_record",
        p.params_in(
            "Mixed Case",
            json!({
                "table": "Order Details",
                "data": {
                    "Order Id": 2, "Unit Price": "3.50", "select": "sel",
                    "Qty#": "4", "O'Brien": "it's", "Größe": "klein"
                }
            }),
        ),
    );
    assert_eq!(inserted, json!(1));

    let row = p.query_ok(
        "SELECT * FROM \"Mixed Case\".\"Order Details\" WHERE \"Order Id\" = 2",
        json!(1),
        1,
    );
    assert_eq!(row["rows"], json!([[2, "3.50", "sel", 4, "it's", "klein"]]));
    p.query_ok(cleanup, Value::Null, 1);
}

#[test]
fn test_insert_record_constraint_violations() {
    require_db2!();
    let mut p = Plugin::spawn();
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.CRUD_TEST WHERE ID BETWEEN 800 AND 899",
        Value::Null,
        1,
    );

    let mut error = |table: &str, data: Value| -> String {
        let resp = p.call(
            "insert_record",
            p.schema_params(json!({ "table": table, "data": data })),
        );
        resp["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {resp}"))
            .to_string()
    };

    // Rejected before reaching Db2: text where a number belongs.
    assert_eq!(
        error("CRUD_TEST", json!({ "ID": "abc", "NAME": "x" })),
        "'abc' is not a valid number for column \"ID\""
    );
    assert_eq!(
        error(
            "CRUD_TEST",
            json!({ "ID": "1; DROP TABLE TEST_SCHEMA.CRUD_TEST", "NAME": "x" })
        ),
        "'1; DROP TABLE TEST_SCHEMA.CRUD_TEST' is not a valid number for column \"ID\""
    );

    // NOT NULL: Db2 identifies the column by internal numbers; the message
    // names it.
    assert_eq!(
        error("CRUD_TEST", json!({ "NAME": "x" })),
        "SQL0407N Assignment of a NULL value to a NOT NULL column \"ID\" is not allowed. SQLSTATE=23502"
    );
    let required = error(
        "EMPLOYEES",
        json!({ "FIRST_NAME": "a", "EMAIL": "nn@example.com", "HIRE_DATE": "2024-01-01" }),
    );
    assert!(
        required.contains("NOT NULL column \"LAST_NAME\""),
        "{required}"
    );

    // Primary key.
    let resp = p.call(
        "insert_record",
        p.schema_params(json!({ "table": "CRUD_TEST", "data": { "ID": 801, "NAME": "x" } })),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
    let mut error = |table: &str, data: Value| -> String {
        let resp = p.call(
            "insert_record",
            p.schema_params(json!({ "table": table, "data": data })),
        );
        resp["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {resp}"))
            .to_string()
    };
    let duplicate = error("CRUD_TEST", json!({ "ID": 801, "NAME": "x" }));
    assert!(duplicate.starts_with("SQL0803N"), "{duplicate}");
    assert!(duplicate.ends_with("SQLSTATE=23505"), "{duplicate}");

    // Unique index.
    let unique = error(
        "EMPLOYEES",
        json!({ "FIRST_NAME": "a", "LAST_NAME": "b", "EMAIL": "john.doe@example.com", "HIRE_DATE": "2024-01-01" }),
    );
    assert!(unique.starts_with("SQL0803N"), "{unique}");

    // Foreign key: the constraint is named.
    let foreign = error(
        "EMPLOYEES",
        json!({ "FIRST_NAME": "a", "LAST_NAME": "b", "EMAIL": "fk@example.com", "HIRE_DATE": "2024-01-01", "DEPT_ID": 999 }),
    );
    assert!(foreign.starts_with("SQL0530N"), "{foreign}");
    assert!(
        foreign.contains("TEST_SCHEMA.EMPLOYEES.FK_EMP_DEPT"),
        "{foreign}"
    );
    assert!(foreign.ends_with("SQLSTATE=23503"), "{foreign}");

    // A value that does not fit, a value for a GENERATED ALWAYS column, a
    // malformed date, an unknown column and an unknown table.
    let long = error("CRUD_TEST", json!({ "ID": 802, "NAME": "x".repeat(200) }));
    assert!(long.contains("SQLSTATE=22001"), "{long}");
    let generated = error(
        "EMPLOYEES",
        json!({ "EMP_ID": 77, "FIRST_NAME": "a", "LAST_NAME": "b", "EMAIL": "id@example.com", "HIRE_DATE": "2024-01-01" }),
    );
    assert!(generated.starts_with("SQL0798N"), "{generated}");
    let date = error(
        "EMPLOYEES",
        json!({ "FIRST_NAME": "a", "LAST_NAME": "b", "EMAIL": "d@example.com", "HIRE_DATE": "not a date" }),
    );
    assert!(date.starts_with("SQL0180N"), "{date}");
    let column = error("CRUD_TEST", json!({ "ID": 803, "NOPE": "x" }));
    assert!(column.starts_with("SQL0206N \"NOPE\""), "{column}");
    let table = error("NOPE_TABLE", json!({ "ID": 1 }));
    assert_eq!(
        table,
        "SQL0204N \"TEST_SCHEMA.NOPE_TABLE\" is an undefined name. SQLSTATE=42704"
    );

    // A BLOB preview (fewer bytes than its header announces) is not written.
    let preview = error(
        "TYPE_METADATA_TEST",
        json!({ "ID": 1005, "COL_BLOB": "BLOB:50000:application/octet-stream:yv66vg==" }),
    );
    assert!(preview.contains("preview"), "{preview}");

    // Nothing was inserted by the failed attempts.
    let count = p.query_ok(
        "SELECT COUNT(*) FROM TEST_SCHEMA.CRUD_TEST WHERE ID BETWEEN 800 AND 899",
        Value::Null,
        1,
    );
    assert_eq!(count["rows"], json!([[1]]));
    let employees = p.query_ok("SELECT COUNT(*) FROM TEST_SCHEMA.EMPLOYEES", Value::Null, 1);
    assert_eq!(employees["rows"], json!([[6]]));

    p.query_ok(
        "DELETE FROM TEST_SCHEMA.CRUD_TEST WHERE ID BETWEEN 800 AND 899",
        Value::Null,
        1,
    );
}

// ---------------------------------------------------------------------------
// Tests — Update record
// ---------------------------------------------------------------------------

/// `update_record` in TEST_SCHEMA; returns the raw response.
fn update(p: &mut Plugin, table: &str, pk_map: Value, column: &str, value: Value) -> Value {
    p.call(
        "update_record",
        p.schema_params(json!({
            "table": table, "pk_map": pk_map, "col_name": column, "new_val": value
        })),
    )
}

fn update_error(p: &mut Plugin, table: &str, pk_map: Value, column: &str, value: Value) -> String {
    let resp = update(p, table, pk_map, column, value);
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {resp}"))
        .to_string()
}

#[test]
fn test_update_record_all_types() {
    require_db2!();
    let mut p = Plugin::spawn();
    delete_inserted_type_rows(&mut p);
    p.call_ok(
        "insert_record",
        p.schema_params(json!({ "table": "TYPE_METADATA_TEST", "data": { "ID": 1010 } })),
    );

    // One column at a time, as the grid does, each value as typed text.
    for (column, value) in [
        ("COL_CHAR", json!("upd")),
        ("COL_VARCHAR", json!("O'Brien — 所有")),
        ("COL_BIT_DATA", json!("0A0B0C")),
        ("COL_GRAPHIC", json!("グラ")),
        ("COL_VARGRAPHIC", json!("变长")),
        ("COL_SMALLINT", json!("-5")),
        ("COL_INTEGER", json!("123456")),
        ("COL_DECIMAL", json!("99.9900")),
        ("COL_DECFLOAT16", json!("2.5")),
        ("COL_DECFLOAT34", json!("0.000000000000000000001")),
        ("COL_REAL", json!("2.5")),
        ("COL_DOUBLE", json!("-0.125")),
        ("COL_BOOLEAN", json!("true")),
        ("COL_DATE", json!("2025-12-31")),
        ("COL_TIME", json!("23:59:59")),
        ("COL_TIMESTAMP", json!("2025-12-31T23:59:59.999999")),
        ("COL_TIMESTAMP12", json!("2025-12-31 23:59:59.999999999999")),
        ("COL_CLOB", json!("x".repeat(40_000))),
        ("COL_DBCLOB", json!("文本")),
        ("COL_BLOB", json!("BLOB:3:application/octet-stream:AQID")),
        ("COL_BINARY", json!("FFEEDDCCBBAA99887766554433221100")),
        ("COL_VARBINARY", json!("00FF")),
        ("COL_XML", json!("<doc id=\"1\"/>")),
    ] {
        let resp = update(
            &mut p,
            "TYPE_METADATA_TEST",
            json!({ "ID": 1010 }),
            column,
            value,
        );
        assert_eq!(resp["result"], json!(1), "{column}: {resp}");
    }

    let row = type_row(&mut p, 1010);
    for (name, expected) in [
        ("COL_CHAR", json!("upd       ")),
        ("COL_VARCHAR", json!("O'Brien — 所有")),
        ("COL_BIT_DATA", json!("0A0B0C")),
        ("COL_GRAPHIC", json!("グラ  ")),
        ("COL_VARGRAPHIC", json!("变长")),
        ("COL_SMALLINT", json!(-5)),
        ("COL_INTEGER", json!(123456)),
        ("COL_DECIMAL", json!("99.9900")),
        ("COL_DECFLOAT16", json!("2.5")),
        ("COL_DECFLOAT34", json!("1E-21")),
        ("COL_REAL", json!(2.5)),
        ("COL_DOUBLE", json!(-0.125)),
        ("COL_BOOLEAN", json!(true)),
        ("COL_DATE", json!("2025-12-31")),
        ("COL_TIME", json!("23:59:59")),
        ("COL_TIMESTAMP", json!("2025-12-31 23:59:59.999999")),
        ("COL_TIMESTAMP12", json!("2025-12-31 23:59:59.999999999999")),
        ("COL_CLOB", json!("x".repeat(40_000))),
        ("COL_DBCLOB", json!("文本")),
        ("COL_BLOB", json!("BLOB:3:application/octet-stream:AQID")),
        ("COL_BINARY", json!("FFEEDDCCBBAA99887766554433221100")),
        ("COL_VARBINARY", json!("00FF")),
        // Recomputed by Db2 from the new COL_VARCHAR (its length in bytes).
        ("COL_COMPUTED", json!(18)),
    ] {
        assert_eq!(row[name], expected, "{name}");
    }
    assert!(row["COL_XML"]
        .as_str()
        .unwrap()
        .ends_with("<doc id=\"1\"/>"));

    // NULL, the column default, and a cleared numeric cell.
    for (column, value, expected) in [
        ("COL_VARCHAR", Value::Null, Value::Null),
        ("COL_BLOB", Value::Null, Value::Null),
        ("COL_CHAR", json!("__USE_DEFAULT__"), json!("N/A       ")),
        ("COL_SMALLINT", json!("__USE_DEFAULT__"), json!(1)),
        ("COL_INTEGER", json!(""), Value::Null),
        ("COL_DATE", json!(""), Value::Null),
    ] {
        let resp = update(
            &mut p,
            "TYPE_METADATA_TEST",
            json!({ "ID": 1010 }),
            column,
            value,
        );
        assert_eq!(resp["result"], json!(1), "{column}: {resp}");
        assert_eq!(type_row(&mut p, 1010)[column], expected, "{column}");
    }

    delete_inserted_type_rows(&mut p);
}

#[test]
fn test_update_record_keys() {
    require_db2!();
    let mut p = Plugin::spawn();

    // Composite primary key: both columns identify the row.
    let role = |p: &mut Plugin| {
        p.query_ok(
            "SELECT EMP_ID, PROJECT_ID, ROLE FROM TEST_SCHEMA.EMP_PROJECTS ORDER BY EMP_ID, PROJECT_ID",
            Value::Null,
            1,
        )["rows"]
            .clone()
    };
    let before = role(&mut p);
    let resp = update(
        &mut p,
        "EMP_PROJECTS",
        json!({ "EMP_ID": 1, "PROJECT_ID": 2 }),
        "ROLE",
        json!("OWNER"),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
    let after = role(&mut p);
    let changed: Vec<_> = before
        .as_array()
        .unwrap()
        .iter()
        .zip(after.as_array().unwrap())
        .filter(|(old, new)| old != new)
        .collect();
    assert_eq!(changed.len(), 1, "exactly one row changed");
    assert_eq!(changed[0].1, &json!([1, 2, "OWNER"]));
    update(
        &mut p,
        "EMP_PROJECTS",
        json!({ "EMP_ID": 1, "PROJECT_ID": 2 }),
        "ROLE",
        json!("MEMBER"),
    );
    assert_eq!(role(&mut p), before);

    // Key values arrive as shown in the grid: a number as text works too.
    let resp = update(
        &mut p,
        "DEPARTMENTS",
        json!({ "DEPT_ID": "4" }),
        "LOCATION",
        json!("Building Z"),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
    update(
        &mut p,
        "DEPARTMENTS",
        json!({ "DEPT_ID": 4 }),
        "LOCATION",
        json!("Building A"),
    );

    // The single key pair Tabularis sent before `pk_map` existed.
    let resp = p.call(
        "update_record",
        p.schema_params(json!({
            "table": "DEPARTMENTS", "pk_col": "DEPT_ID", "pk_val": 4,
            "col_name": "LOCATION", "new_val": "Building A"
        })),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");

    // Changing the key column itself.
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.CRUD_TEST WHERE ID IN (910, 911)",
        Value::Null,
        1,
    );
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.CRUD_TEST (ID, NAME) VALUES (910, 'moved')",
        Value::Null,
        1,
    );
    let resp = update(&mut p, "CRUD_TEST", json!({ "ID": 910 }), "ID", json!(911));
    assert_eq!(resp["result"], json!(1), "{resp}");
    let moved = p.query_ok(
        "SELECT ID FROM TEST_SCHEMA.CRUD_TEST WHERE NAME = 'moved'",
        Value::Null,
        1,
    );
    assert_eq!(moved["rows"], json!([[911]]));
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.CRUD_TEST WHERE ID IN (910, 911)",
        Value::Null,
        1,
    );

    // Delimited names in the table, the key and the column.
    let resp = p.call(
        "update_record",
        p.params_in(
            "Mixed Case",
            json!({
                "table": "Order Details", "pk_map": { "Order Id": 1 },
                "col_name": "O'Brien", "new_val": "it's \"quoted\""
            }),
        ),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
    let row = p.query_ok(
        "SELECT \"O'Brien\" FROM \"Mixed Case\".\"Order Details\" WHERE \"Order Id\" = 1",
        Value::Null,
        1,
    );
    assert_eq!(row["rows"], json!([["it's \"quoted\""]]));
    p.call_ok(
        "update_record",
        p.params_in(
            "Mixed Case",
            json!({
                "table": "Order Details", "pk_map": { "Order Id": 1 },
                "col_name": "O'Brien", "new_val": "it's"
            }),
        ),
    );
}

#[test]
fn test_update_record_table_without_primary_key() {
    require_db2!();
    let mut p = Plugin::spawn();
    let rows = |p: &mut Plugin| {
        p.query_ok(
            "SELECT CODE, QTY, CAST(NOTE AS VARCHAR(100)), CREATED FROM TEST_SCHEMA.KEYLESS_TEST ORDER BY CODE, QTY",
            Value::Null,
            1,
        )["rows"]
            .clone()
    };
    let original = rows(&mut p);
    assert_eq!(
        original,
        json!([
            ["A", 1, "first", "2024-01-01"],
            ["A", 1, "first", "2024-01-01"],
            ["B", null, null, null],
            ["C", 3, "third", "2024-03-03"]
        ])
    );

    // Without any key the row cannot be identified: a clear error instead
    // of the invalid `WHERE "" = ...` this used to generate (issue #4).
    for keys in [json!({}), Value::Null] {
        let message = update_error(&mut p, "KEYLESS_TEST", keys, "QTY", json!(5));
        assert_eq!(
            message,
            "Cannot identify the row in \"TEST_SCHEMA\".\"KEYLESS_TEST\": no key values were \
             provided. The table needs a primary key, or every column of the row must be loaded."
        );
    }
    let resp = p.call(
        "update_record",
        p.schema_params(json!({
            "table": "KEYLESS_TEST", "pk_col": "", "pk_val": null, "col_name": "QTY", "new_val": 5
        })),
    );
    assert!(resp["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Cannot identify the row"));
    assert_eq!(rows(&mut p), original, "nothing changed");

    // What Tabularis sends for a keyless table: every column of the row.
    // Two rows are identical; only one of them is updated. The CLOB is not
    // comparable and is left out of the match.
    let twin = json!({ "CODE": "A", "QTY": 1, "NOTE": "first", "CREATED": "2024-01-01" });
    let resp = update(&mut p, "KEYLESS_TEST", twin.clone(), "QTY", json!("10"));
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(
        rows(&mut p),
        json!([
            ["A", 1, "first", "2024-01-01"],
            ["A", 10, "first", "2024-01-01"],
            ["B", null, null, null],
            ["C", 3, "third", "2024-03-03"]
        ])
    );

    // NULL key values are matched with IS NULL.
    let nulls = json!({ "CODE": "B", "QTY": null, "NOTE": null, "CREATED": null });
    let resp = update(&mut p, "KEYLESS_TEST", nulls, "NOTE", json!("filled in"));
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(rows(&mut p)[2], json!(["B", null, "filled in", null]));

    // Restore the fixture rows.
    let changed = json!({ "CODE": "A", "QTY": 10, "NOTE": "first", "CREATED": "2024-01-01" });
    update(&mut p, "KEYLESS_TEST", changed, "QTY", json!(1));
    let filled = json!({ "CODE": "B", "QTY": null, "CREATED": null });
    update(&mut p, "KEYLESS_TEST", filled, "NOTE", Value::Null);
    assert_eq!(rows(&mut p), original);
}

#[test]
fn test_update_record_errors() {
    require_db2!();
    let mut p = Plugin::spawn();
    let employee = json!({ "EMP_ID": 1 });
    let snapshot = |p: &mut Plugin| {
        p.query_ok(
            "SELECT * FROM TEST_SCHEMA.EMPLOYEES ORDER BY EMP_ID",
            Value::Null,
            1,
        )["rows"]
            .clone()
    };
    let before = snapshot(&mut p);

    // A key that matches no row.
    assert_eq!(
        update_error(
            &mut p,
            "EMPLOYEES",
            json!({ "EMP_ID": 424242 }),
            "LAST_NAME",
            json!("x")
        ),
        "No row in \"TEST_SCHEMA\".\"EMPLOYEES\" matches the key values: it was changed or \
         deleted after it was loaded. Refresh the table and try again."
    );

    // Constraint violations, each with Db2's message.
    let foreign = update_error(&mut p, "EMPLOYEES", employee.clone(), "DEPT_ID", json!(999));
    assert!(foreign.starts_with("SQL0530N"), "{foreign}");
    assert!(foreign.contains("FK_EMP_DEPT"), "{foreign}");

    let unique = update_error(
        &mut p,
        "EMPLOYEES",
        employee.clone(),
        "EMAIL",
        json!("jane.smith@example.com"),
    );
    assert!(unique.starts_with("SQL0803N"), "{unique}");

    assert_eq!(
        update_error(&mut p, "EMPLOYEES", employee.clone(), "LAST_NAME", Value::Null),
        "SQL0407N Assignment of a NULL value to a NOT NULL column \"LAST_NAME\" is not allowed. SQLSTATE=23502"
    );

    let long = update_error(
        &mut p,
        "EMPLOYEES",
        employee.clone(),
        "FIRST_NAME",
        json!("x".repeat(60)),
    );
    assert!(long.contains("SQLSTATE=22001"), "{long}");

    // Values that are refused before reaching Db2.
    assert_eq!(
        update_error(
            &mut p,
            "EMPLOYEES",
            employee.clone(),
            "SALARY",
            json!("lots")
        ),
        "'lots' is not a valid number for column \"SALARY\""
    );
    assert_eq!(
        update_error(
            &mut p,
            "EMPLOYEES",
            json!({ "EMP_ID": "1 OR 1=1" }),
            "SALARY",
            json!(1)
        ),
        "'1 OR 1=1' is not a valid number for column \"EMP_ID\""
    );

    let generated = update_error(&mut p, "EMPLOYEES", employee.clone(), "EMP_ID", json!(50));
    assert!(generated.starts_with("SQL0798N"), "{generated}");
    let column = update_error(&mut p, "EMPLOYEES", employee.clone(), "NOPE", json!(1));
    assert!(column.starts_with("SQL0206N \"NOPE\""), "{column}");
    let table = update_error(&mut p, "NOPE_TABLE", json!({ "ID": 1 }), "X", json!(1));
    assert!(
        table.starts_with("SQL0204N \"TEST_SCHEMA.NOPE_TABLE\""),
        "{table}"
    );

    // Missing arguments are invalid requests.
    let resp = p.call(
        "update_record",
        p.schema_params(json!({ "table": "EMPLOYEES", "pk_map": employee, "new_val": 1 })),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");

    assert_eq!(
        snapshot(&mut p),
        before,
        "no failed update changed anything"
    );
}

// ---------------------------------------------------------------------------
// Tests — Delete record
// ---------------------------------------------------------------------------

/// `delete_record` in TEST_SCHEMA; returns the raw response.
fn delete(p: &mut Plugin, table: &str, pk_map: Value) -> Value {
    p.call(
        "delete_record",
        p.schema_params(json!({ "table": table, "pk_map": pk_map })),
    )
}

fn delete_error(p: &mut Plugin, table: &str, pk_map: Value) -> String {
    let resp = delete(p, table, pk_map);
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {resp}"))
        .to_string()
}

fn count(p: &mut Plugin, table: &str, condition: &str) -> i64 {
    p.query_ok(
        &format!("SELECT COUNT(*) FROM TEST_SCHEMA.{table} WHERE {condition}"),
        Value::Null,
        1,
    )["rows"][0][0]
        .as_i64()
        .unwrap()
}

#[test]
fn test_delete_record_by_primary_key() {
    require_db2!();
    let mut p = Plugin::spawn();
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.CRUD_TEST WHERE ID BETWEEN 920 AND 929",
        Value::Null,
        1,
    );
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.CRUD_TEST (ID, NAME) VALUES (920, 'a'), (921, 'b'), (922, 'c'), (923, 'd')",
        Value::Null,
        1,
    );

    // A single-column key, as a number and as the text the grid may send.
    let resp = delete(&mut p, "CRUD_TEST", json!({ "ID": 920 }));
    assert_eq!(resp["result"], json!(1), "{resp}");
    let resp = delete(&mut p, "CRUD_TEST", json!({ "ID": "921" }));
    assert_eq!(resp["result"], json!(1), "{resp}");

    // The single key pair Tabularis sent before `pk_map` existed.
    let resp = p.call(
        "delete_record",
        p.schema_params(json!({ "table": "CRUD_TEST", "pk_col": "ID", "pk_val": 922 })),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");

    // Only the rows asked for are gone.
    let left = p.query_ok(
        "SELECT ID FROM TEST_SCHEMA.CRUD_TEST WHERE ID BETWEEN 920 AND 929",
        Value::Null,
        1,
    );
    assert_eq!(left["rows"], json!([[923]]));
    delete(&mut p, "CRUD_TEST", json!({ "ID": 923 }));

    // Composite primary key: both columns identify the row.
    let before = count(&mut p, "EMP_PROJECTS", "1 = 1");
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.EMP_PROJECTS (EMP_ID, PROJECT_ID, ROLE) VALUES (5, 3, 'TMP')",
        Value::Null,
        1,
    );
    let resp = delete(
        &mut p,
        "EMP_PROJECTS",
        json!({ "EMP_ID": 5, "PROJECT_ID": 3 }),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(count(&mut p, "EMP_PROJECTS", "1 = 1"), before);
    assert_eq!(
        count(&mut p, "EMP_PROJECTS", "PROJECT_ID = 3"),
        1,
        "the other row of project 3 stays"
    );

    // Delimited names in the schema, the table and the key.
    p.query_ok(
        "INSERT INTO \"Mixed Case\".\"Order Details\" (\"Order Id\", \"Unit Price\") VALUES (9, 1)",
        Value::Null,
        1,
    );
    let resp = p.call(
        "delete_record",
        p.params_in(
            "Mixed Case",
            json!({ "table": "Order Details", "pk_map": { "Order Id": 9 } }),
        ),
    );
    assert_eq!(resp["result"], json!(1), "{resp}");
}

#[test]
fn test_delete_record_table_without_primary_key() {
    require_db2!();
    let mut p = Plugin::spawn();
    let rows = |p: &mut Plugin| {
        p.query_ok(
            "SELECT CODE, QTY FROM TEST_SCHEMA.KEYLESS_TEST ORDER BY CODE, QTY",
            Value::Null,
            1,
        )["rows"]
            .clone()
    };
    assert_eq!(
        rows(&mut p),
        json!([["A", 1], ["A", 1], ["B", null], ["C", 3]])
    );

    // Two identical rows: deleting "the row" removes one of them.
    let twin = json!({ "CODE": "A", "QTY": 1, "NOTE": "first", "CREATED": "2024-01-01" });
    let resp = delete(&mut p, "KEYLESS_TEST", twin.clone());
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(rows(&mut p), json!([["A", 1], ["B", null], ["C", 3]]));

    // A row identified partly by NULLs.
    let nulls = json!({ "CODE": "B", "QTY": null, "NOTE": null, "CREATED": null });
    let resp = delete(&mut p, "KEYLESS_TEST", nulls);
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(rows(&mut p), json!([["A", 1], ["C", 3]]));

    // Without key values nothing is deleted.
    let message = delete_error(&mut p, "KEYLESS_TEST", json!({}));
    assert!(
        message.starts_with("Cannot identify the row in \"TEST_SCHEMA\".\"KEYLESS_TEST\""),
        "{message}"
    );
    assert_eq!(rows(&mut p), json!([["A", 1], ["C", 3]]));

    // Restore the fixture rows.
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.KEYLESS_TEST (CODE, QTY, NOTE, CREATED) VALUES ('A', 1, 'first', '2024-01-01'), ('B', NULL, NULL, NULL)",
        Value::Null,
        1,
    );
    assert_eq!(
        rows(&mut p),
        json!([["A", 1], ["A", 1], ["B", null], ["C", 3]])
    );
}

#[test]
fn test_delete_record_foreign_keys() {
    require_db2!();
    let mut p = Plugin::spawn();

    // ON DELETE RESTRICT: a parent that still has children cannot go, and
    // Db2 names the constraint.
    let message = delete_error(&mut p, "FK_PARENT", json!({ "ID": 1 }));
    assert!(message.starts_with("SQL0532N"), "{message}");
    assert!(
        message.contains("TEST_SCHEMA.FK_CHILD.FK_CHILD_PARENT"),
        "{message}"
    );
    assert!(message.ends_with("SQLSTATE=23001"), "{message}");
    assert_eq!(count(&mut p, "FK_PARENT", "ID = 1"), 1, "parent kept");
    assert_eq!(
        count(&mut p, "FK_CHILD", "PARENT_ID = 1"),
        2,
        "children kept"
    );

    // A parent without children can.
    let resp = delete(&mut p, "FK_PARENT", json!({ "ID": 2 }));
    assert_eq!(resp["result"], json!(1), "{resp}");
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.FK_PARENT (ID, NAME) VALUES (2, 'no children')",
        Value::Null,
        1,
    );

    // Once its children are deleted, so can the first one.
    for child in [10, 11] {
        let resp = delete(&mut p, "FK_CHILD", json!({ "ID": child }));
        assert_eq!(resp["result"], json!(1), "{resp}");
    }
    let resp = delete(&mut p, "FK_PARENT", json!({ "ID": 1 }));
    assert_eq!(resp["result"], json!(1), "{resp}");
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.FK_PARENT (ID, NAME) VALUES (1, 'has children')",
        Value::Null,
        1,
    );
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.FK_CHILD (ID, PARENT_ID) VALUES (10, 1), (11, 1)",
        Value::Null,
        1,
    );

    // ON DELETE CASCADE and SET NULL: the delete succeeds and Db2 applies
    // the rule to the dependent rows.
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.DEPARTMENTS (DEPT_NAME) VALUES ('Temp dept')",
        Value::Null,
        1,
    );
    let dept = p.query_ok(
        "SELECT DEPT_ID FROM TEST_SCHEMA.DEPARTMENTS WHERE DEPT_NAME = 'Temp dept'",
        Value::Null,
        1,
    )["rows"][0][0]
        .as_i64()
        .unwrap();
    p.query_ok(
        &format!("INSERT INTO TEST_SCHEMA.PROJECTS (PROJECT_NAME, DEPT_ID) VALUES ('Temp project', {dept})"),
        Value::Null,
        1,
    );
    p.query_ok(
        &format!("INSERT INTO TEST_SCHEMA.EMPLOYEES (FIRST_NAME, LAST_NAME, EMAIL, HIRE_DATE, DEPT_ID) VALUES ('Temp', 'Person', 'temp.person@example.com', '2024-01-01', {dept})"),
        Value::Null,
        1,
    );

    let resp = delete(&mut p, "DEPARTMENTS", json!({ "DEPT_ID": dept }));
    assert_eq!(resp["result"], json!(1), "{resp}");
    assert_eq!(
        count(&mut p, "PROJECTS", "PROJECT_NAME = 'Temp project'"),
        0,
        "cascade"
    );
    assert_eq!(
        count(
            &mut p,
            "EMPLOYEES",
            "EMAIL = 'temp.person@example.com' AND DEPT_ID IS NULL"
        ),
        1,
        "set null"
    );
    p.query_ok(
        "DELETE FROM TEST_SCHEMA.EMPLOYEES WHERE EMAIL = 'temp.person@example.com'",
        Value::Null,
        1,
    );
    assert_eq!(count(&mut p, "EMPLOYEES", "1 = 1"), 6);
    assert_eq!(count(&mut p, "DEPARTMENTS", "1 = 1"), 4);
}

#[test]
fn test_delete_record_errors() {
    require_db2!();
    let mut p = Plugin::spawn();
    let before = count(&mut p, "EMPLOYEES", "1 = 1");

    // A key that matches no row: the row is already gone.
    assert_eq!(
        delete_error(&mut p, "EMPLOYEES", json!({ "EMP_ID": 424242 })),
        "No row in \"TEST_SCHEMA\".\"EMPLOYEES\" matches the key values: it was changed or \
         deleted after it was loaded. Refresh the table and try again."
    );

    // No key values, in every shape that can arrive.
    for keys in [json!({}), Value::Null, json!({ "": 1 })] {
        let message = delete_error(&mut p, "EMPLOYEES", keys);
        assert!(message.starts_with("Cannot identify the row"), "{message}");
    }
    let resp = p.call(
        "delete_record",
        p.schema_params(json!({ "table": "EMPLOYEES", "pk_col": "", "pk_val": null })),
    );
    assert!(resp["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Cannot identify the row"));

    // A key value that is not what its column holds never reaches Db2 as SQL.
    assert_eq!(
        delete_error(&mut p, "EMPLOYEES", json!({ "EMP_ID": "1 OR 1=1" })),
        "'1 OR 1=1' is not a valid number for column \"EMP_ID\""
    );
    // In a text key it is just text that matches nothing.
    let message = delete_error(&mut p, "EMPLOYEES", json!({ "EMAIL": "' OR '1'='1" }));
    assert!(message.starts_with("No row in"), "{message}");

    // Unknown key column, unknown table, missing table argument.
    let column = delete_error(&mut p, "EMPLOYEES", json!({ "NOPE": 1 }));
    assert!(column.starts_with("SQL0206N \"NOPE\""), "{column}");
    let table = delete_error(&mut p, "NOPE_TABLE", json!({ "ID": 1 }));
    assert!(
        table.starts_with("SQL0204N \"TEST_SCHEMA.NOPE_TABLE\""),
        "{table}"
    );
    let resp = p.call(
        "delete_record",
        p.schema_params(json!({ "pk_map": { "ID": 1 } })),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");

    assert_eq!(
        count(&mut p, "EMPLOYEES", "1 = 1"),
        before,
        "nothing was deleted"
    );
}

// ---------------------------------------------------------------------------
// Tests — Drop operations (uses DROP_TEST table)
// ---------------------------------------------------------------------------

#[test]
fn test_drop_index() {
    require_db2!();
    let mut p = Plugin::spawn();
    p.query("DROP INDEX TEST_SCHEMA.IDX_T_DROP", Value::Null, 1);
    p.query_ok(
        "CREATE INDEX TEST_SCHEMA.IDX_T_DROP ON TEST_SCHEMA.DROP_TEST (NAME, PARENT_ID)",
        Value::Null,
        1,
    );
    let has_index = |p: &mut Plugin| {
        index_entries(p, "DROP_TEST")
            .iter()
            .any(|entry| entry[0] == json!("IDX_T_DROP"))
    };
    assert!(has_index(&mut p));
    let drop = |p: &mut Plugin, table: &str, index: &str| {
        p.call(
            "drop_index",
            p.schema_params(json!({ "table": table, "index_name": index })),
        )
    };

    // An index of another table is not dropped by mistake.
    let resp = drop(&mut p, "EMPLOYEES", "IDX_T_DROP");
    assert_eq!(
        resp["error"]["message"],
        json!("Index \"IDX_T_DROP\" was not found on table \"TEST_SCHEMA\".\"EMPLOYEES\"")
    );
    assert!(has_index(&mut p));

    // Dropped, and gone from the list.
    let resp = drop(&mut p, "DROP_TEST", "IDX_T_DROP");
    assert_eq!(resp["result"], Value::Null, "{resp}");
    assert!(resp.get("error").is_none(), "{resp}");
    assert!(!has_index(&mut p));

    // An index that does not exist (any more).
    let resp = drop(&mut p, "DROP_TEST", "IDX_T_DROP");
    assert_eq!(
        resp["error"]["message"],
        json!("Index \"IDX_T_DROP\" was not found on table \"TEST_SCHEMA\".\"DROP_TEST\"")
    );
    let resp = drop(&mut p, "NOPE_TABLE", "IDX_T_DROP");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("was not found"),
        "{resp}"
    );
    let resp = p.call(
        "drop_index",
        p.schema_params(json!({ "table": "DROP_TEST" })),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");

    // The index behind a primary key cannot be dropped on its own.
    let primary = index_entries(&mut p, "DROP_TEST")
        .into_iter()
        .find(|entry| entry[3] == json!(true))
        .expect("primary key index");
    let resp = drop(&mut p, "DROP_TEST", primary[0].as_str().unwrap());
    assert_eq!(
        resp["error"]["message"],
        json!("SQL0669N A system required index cannot be dropped explicitly. SQLSTATE=42917")
    );
    assert!(index_entries(&mut p, "DROP_TEST")
        .iter()
        .any(|entry| entry[3] == json!(true)));
}

#[test]
fn test_drop_foreign_key() {
    require_db2!();
    let mut p = Plugin::spawn();
    // A table of its own, with two foreign keys on different columns (Db2
    // ignores a second constraint on the same columns as a duplicate).
    p.query("DROP TABLE TEST_SCHEMA.FK_TMP_CHILD", Value::Null, 1);
    p.query_ok(
        "CREATE TABLE TEST_SCHEMA.FK_TMP_CHILD (A INTEGER, B INTEGER, \
         CONSTRAINT FK_T_DROP FOREIGN KEY (A) REFERENCES TEST_SCHEMA.FK_PARENT (ID) ON DELETE CASCADE, \
         CONSTRAINT \"FK \"\"odd\"\" name\" FOREIGN KEY (B) REFERENCES TEST_SCHEMA.DROP_TEST_PARENT (ID))",
        Value::Null,
        1,
    );
    let names = |p: &mut Plugin| -> Vec<String> {
        foreign_keys_of(p, "FK_TMP_CHILD")
            .iter()
            .map(|key| key[0].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&mut p), ["FK \"odd\" name", "FK_T_DROP"]);
    let drop = |p: &mut Plugin, table: &str, args: Value| {
        let mut params = json!({ "table": table });
        for (key, value) in args.as_object().unwrap() {
            params[key] = value.clone();
        }
        p.call("drop_foreign_key", p.schema_params(params))
    };

    // While the constraint exists it is enforced.
    let orphan = p.query(
        "INSERT INTO TEST_SCHEMA.FK_TMP_CHILD (A) VALUES (999)",
        Value::Null,
        1,
    );
    assert!(orphan["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("SQL0530N"));

    // A foreign key of another table is not this table's to drop.
    let resp = drop(&mut p, "EMPLOYEES", json!({ "fk_name": "FK_T_DROP" }));
    assert_eq!(
        resp["error"]["message"],
        json!("Foreign key \"FK_T_DROP\" was not found on table \"TEST_SCHEMA\".\"EMPLOYEES\"")
    );
    assert_eq!(names(&mut p).len(), 2);

    // Dropped, by the argument name Tabularis sends and by the documented
    // one, with a name that needs quoting.
    let resp = drop(&mut p, "FK_TMP_CHILD", json!({ "fk_name": "FK_T_DROP" }));
    assert_eq!(resp["result"], Value::Null, "{resp}");
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(names(&mut p), ["FK \"odd\" name"]);
    let resp = drop(
        &mut p,
        "FK_TMP_CHILD",
        json!({ "constraint_name": "FK \"odd\" name" }),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(names(&mut p), Vec::<String>::new());

    // The constraint no longer restricts anything.
    p.query_ok(
        "INSERT INTO TEST_SCHEMA.FK_TMP_CHILD (A) VALUES (999)",
        Value::Null,
        1,
    );

    // A foreign key that does not exist (any more), a missing table and a
    // missing argument.
    let resp = drop(&mut p, "FK_TMP_CHILD", json!({ "fk_name": "FK_T_DROP" }));
    assert_eq!(
        resp["error"]["message"],
        json!("Foreign key \"FK_T_DROP\" was not found on table \"TEST_SCHEMA\".\"FK_TMP_CHILD\"")
    );
    let resp = drop(&mut p, "NOPE_TABLE", json!({ "fk_name": "FK_T_DROP" }));
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("was not found"),
        "{resp}"
    );
    let resp = drop(&mut p, "FK_TMP_CHILD", json!({}));
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp}");

    // A primary key is a constraint but not a foreign key.
    let primary = index_entries(&mut p, "FK_PARENT")[0][0]
        .as_str()
        .unwrap()
        .to_string();
    let resp = drop(&mut p, "FK_PARENT", json!({ "fk_name": primary }));
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Foreign key"),
        "{resp}"
    );

    p.query_ok("DROP TABLE TEST_SCHEMA.FK_TMP_CHILD", Value::Null, 1);
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
