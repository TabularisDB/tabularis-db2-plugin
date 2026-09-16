use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub ssl_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TableColumn {
    pub name: String,
    pub data_type: String,
    pub is_pk: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    pub default_value: Option<String>,
    pub character_maximum_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForeignKey {
    pub name: String,
    pub column_name: String,
    pub ref_table: String,
    pub ref_column: String,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Index {
    pub name: String,
    pub column_name: String,
    pub is_unique: bool,
    pub is_primary: bool,
    pub seq_in_index: i32,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExplainNode {
    pub id: String,
    pub node_type: String,
    pub relation: Option<String>,
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub plan_rows: Option<f64>,
    pub actual_rows: Option<f64>,
    pub actual_time_ms: Option<f64>,
    pub actual_loops: Option<u64>,
    pub buffers_hit: Option<u64>,
    pub buffers_read: Option<u64>,
    pub filter: Option<String>,
    pub index_condition: Option<String>,
    pub join_type: Option<String>,
    pub hash_condition: Option<String>,
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub children: Vec<ExplainNode>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExplainPlan {
    pub root: ExplainNode,
    pub planning_time_ms: Option<f64>,
    pub execution_time_ms: Option<f64>,
    pub original_query: String,
    pub driver: String,
    pub has_analyze_data: bool,
    pub raw_output: Option<String>,
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
    pub routine_type: String,
    pub definition: Option<String>,
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
    pub default_value: Option<String>,
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
