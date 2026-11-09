use serde_json::Value;

/// Renders text as a Db2 string constant (`O'Brien` -> `'O''Brien'`).
pub fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn json_to_sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(flag) => {
            if *flag {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        Value::Number(number) => number.to_string(),
        Value::String(string) => string_literal(string),
        Value::Array(_) | Value::Object(_) => string_literal(&value.to_string()),
    }
}

pub fn text_row_to_json(value: Option<&str>) -> Value {
    match value {
        None => Value::Null,
        Some(raw) => Value::String(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{json_to_sql_literal, string_literal, text_row_to_json};
    use serde_json::json;

    #[test]
    fn serializes_sql_literals() {
        assert_eq!(json_to_sql_literal(&json!(null)), "NULL");
        assert_eq!(json_to_sql_literal(&json!(true)), "1");
        assert_eq!(json_to_sql_literal(&json!("O'Reilly")), "'O''Reilly'");
    }

    #[test]
    fn quotes_string_constants() {
        assert_eq!(string_literal("TEST_SCHEMA"), "'TEST_SCHEMA'");
        assert_eq!(string_literal("O'Brien"), "'O''Brien'");
        assert_eq!(string_literal(""), "''");
    }

    #[test]
    fn converts_text_rows_to_json() {
        assert_eq!(text_row_to_json(None), serde_json::Value::Null);
        assert_eq!(text_row_to_json(Some("abc")), json!("abc"));
    }
}
