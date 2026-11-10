use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum DatabaseSelection {
    Single(String),
    Multiple(Vec<String>),
}

impl DatabaseSelection {
    pub fn primary(&self) -> &str {
        match self {
            Self::Single(value) => value.as_str(),
            Self::Multiple(values) => values.first().map(String::as_str).unwrap_or(""),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ConnectionParams {
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: DatabaseSelection,
    #[serde(default)]
    pub ssl_mode: Option<String>,
    /// Seconds to wait for a new connection, when the user set one.
    #[serde(default)]
    pub connect_timeout_secs: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TableColumn {
    pub name: String,
    /// Base type name (`VARCHAR`, `DECIMAL`); the host matches on it for
    /// value handling, editing and DDL generation.
    pub data_type: String,
    pub is_pk: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    /// Column computed by Db2 (`GENERATED ALWAYS AS (...)`, row change
    /// timestamp): the host leaves it out of inserts.
    #[serde(default)]
    pub is_generated: bool,
    pub default_value: Option<String>,
    /// Declared length, for string and binary types only.
    pub character_maximum_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_precision: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_scale: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Declared type with its modifiers (`VARCHAR(50)`, `DECIMAL(10,2)`),
    /// for display. Sent only when it says more than `data_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_type: Option<String>,
    /// The default as SQL that can follow `DEFAULT` unchanged, so the column
    /// editor can restate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_sql: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForeignKey {
    pub name: String,
    pub column_name: String,
    pub ref_table: String,
    pub ref_column: String,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
    /// 1-based position of this column within its constraint, so Tabularis
    /// can put the columns of a composite foreign key back in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_in_fk: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Index {
    pub name: String,
    pub column_name: String,
    pub is_unique: bool,
    pub is_primary: bool,
    pub seq_in_index: i32,
    /// True when `column_name` holds an expression rather than a column.
    #[serde(default)]
    pub is_expression: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Pagination {
    pub page: u32,
    pub page_size: u32,
    pub total_rows: Option<u64>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub affected_rows: u64,
    #[serde(default)]
    pub truncated: bool,
    pub pagination: Option<Pagination>,
}

/// One statement's outcome within `execute_query_batch`: exactly one of
/// `result` and `error` is set.
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchStatementResult {
    pub result: Option<QueryResult>,
    pub error: Option<String>,
    pub execution_time_ms: Option<f64>,
}

/// `explain_query`'s answer in Tabularis's raw form: the plan as Db2 wrote
/// it, for the parser this plugin ships in `explain/` to turn into a tree.
#[derive(Debug, Serialize)]
pub struct RawExplain {
    pub engine: &'static str,
    pub format: &'static str,
    /// [`Db2ExplainPayload`] as JSON.
    pub payload: String,
    pub original_query: String,
}

/// The rows Db2's `EXPLAIN` wrote for one statement, one list per explain
/// table, with values as Db2 gives them. `explain/src/payload.ts` mirrors
/// this shape: change both together.
#[derive(Debug, Serialize, Default)]
pub struct Db2ExplainPayload {
    /// Version of this shape; always the first key, so a parser can tell a
    /// Db2 payload from the first bytes.
    pub db2_explain: u32,
    /// The schema the statement was explained in. Objects of that schema are
    /// named without it.
    pub schema: String,
    pub operators: Vec<ExplainOperator>,
    pub streams: Vec<ExplainStream>,
    pub predicates: Vec<ExplainPredicate>,
    pub arguments: Vec<ExplainArgument>,
    pub indexes: Vec<ExplainIndex>,
    /// What running the statement showed, when Analyze was asked for.
    pub analyze: Option<ExplainAnalyze>,
}

/// One `EXPLAIN_OPERATOR` row.
#[derive(Debug, Serialize, Default)]
pub struct ExplainOperator {
    pub id: i64,
    #[serde(rename = "type")]
    pub operator_type: String,
    pub total_cost: Option<f64>,
    pub io_cost: Option<f64>,
    pub cpu_cost: Option<f64>,
    pub first_row_cost: Option<f64>,
    pub re_total_cost: Option<f64>,
    pub buffers: Option<f64>,
}

/// One `EXPLAIN_STREAM` row ending in an operator. `source_type` is `O` for
/// an operator and `D` for a data object (a table or an index, named by
/// `object_schema` and `object_name`, with source id -1).
#[derive(Debug, Serialize)]
pub struct ExplainStream {
    pub source_type: String,
    pub source_id: i64,
    pub target_id: i64,
    pub object_schema: Option<String>,
    pub object_name: Option<String>,
    /// Estimated number of rows.
    pub count: Option<f64>,
}

/// One `EXPLAIN_PREDICATE` row.
#[derive(Debug, Serialize)]
pub struct ExplainPredicate {
    pub operator_id: i64,
    pub how_applied: String,
    pub text: String,
}

/// One `EXPLAIN_ARGUMENT` row.
#[derive(Debug, Serialize)]
pub struct ExplainArgument {
    pub operator_id: i64,
    #[serde(rename = "type")]
    pub argument_type: String,
    pub value: String,
}

/// An index the plan reads, with its table (from `SYSCAT.INDEXES`).
#[derive(Debug, Serialize)]
pub struct ExplainIndex {
    pub schema: String,
    pub name: String,
    pub table_schema: String,
    pub table_name: String,
}

#[derive(Debug, Serialize)]
pub struct ExplainAnalyze {
    /// Rows the statement returned, or changed when it returns none.
    pub rows: u64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<TableColumn>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoutineInfo {
    pub name: String,
    /// `FUNCTION` or `PROCEDURE`.
    pub routine_type: String,
    /// Left out of the list: Tabularis asks for the definition of the routine
    /// it opens.
    pub definition: Option<String>,
    /// The argument types of the routine. Db2 lets routines share a name, so
    /// Tabularis shows this next to it and sends it back to say which one it
    /// means. Empty for a routine without arguments.
    pub identity_args: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoutineParameter {
    pub name: String,
    pub data_type: String,
    pub mode: String,
    pub ordinal_position: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ViewInfo {
    pub name: String,
    pub definition: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColumnDefinition {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_pk: bool,
    pub is_auto_increment: bool,
    #[serde(default)]
    pub default_value: Option<String>,
    /// Column description; only sent when the user entered one.
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PluginSettings {
    pub driver_name: Option<String>,
    pub security: Option<String>,
    pub current_schema: Option<String>,
    pub extra_properties: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{TableColumn, TableInfo};
    use serde_json::json;

    const TABLE_COMMENT: &str = "Employee's résumé\n所有员工";
    const COLUMN_COMMENT: &str = "Manager's notes — café\n第二行";

    #[test]
    fn table_info_round_trips_special_comments_and_omits_missing_comments() {
        let table: TableInfo = serde_json::from_value(json!({
            "name": "EMPLOYEES",
            "comment": TABLE_COMMENT,
        }))
        .expect("deserialize table comment");

        assert_eq!(table.comment.as_deref(), Some(TABLE_COMMENT));
        assert_eq!(
            serde_json::to_value(&table).expect("serialize table comment"),
            json!({ "name": "EMPLOYEES", "comment": TABLE_COMMENT })
        );

        let plain: TableInfo = serde_json::from_value(json!({ "name": "DEPARTMENTS" }))
            .expect("deserialize table without comment");
        assert_eq!(plain.comment, None);
        assert_eq!(
            serde_json::to_value(&plain).expect("serialize table without comment"),
            json!({ "name": "DEPARTMENTS" })
        );
    }

    #[test]
    fn table_column_round_trips_special_comments_and_omits_missing_comments() {
        let column_json = json!({
            "name": "NOTES",
            "data_type": "CLOB",
            "is_pk": false,
            "is_nullable": true,
            "is_auto_increment": false,
            "is_generated": false,
            "default_value": null,
            "character_maximum_length": 1048576,
            "comment": COLUMN_COMMENT,
        });
        let column: TableColumn =
            serde_json::from_value(column_json.clone()).expect("deserialize column comment");

        assert_eq!(column.comment.as_deref(), Some(COLUMN_COMMENT));
        assert_eq!(
            serde_json::to_value(&column).expect("serialize column comment"),
            column_json
        );

        let mut plain_json = column_json;
        plain_json
            .as_object_mut()
            .expect("column object")
            .remove("comment");
        let plain: TableColumn =
            serde_json::from_value(plain_json.clone()).expect("deserialize column without comment");
        assert_eq!(plain.comment, None);
        assert_eq!(
            serde_json::to_value(&plain).expect("serialize column without comment"),
            plain_json
        );
    }
}
