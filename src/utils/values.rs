use crate::client::ColumnKind;
use serde_json::Value;

/// Renders text as a Db2 string constant (`O'Brien` -> `'O''Brien'`).
pub fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Converts the text Db2 rendered for a cell into the JSON value Tabularis
/// gets, according to the column's kind.
///
/// Integers become JSON integers (Tabularis reads `COUNT(*)` with
/// `as_i64`, and a `"6"` string counted as 0), floating-point values JSON
/// numbers and booleans JSON booleans. A value that does not parse as its
/// kind (a `DOUBLE` holding `NaN` or `Infinity`, which JSON cannot express)
/// stays the text Db2 produced rather than being dropped.
pub fn cell_to_json(value: Option<&str>, kind: ColumnKind) -> Value {
    let Some(raw) = value else {
        return Value::Null;
    };
    let text = raw.trim();
    let typed = match kind {
        ColumnKind::Integer => text.parse::<i64>().ok().map(Value::from),
        ColumnKind::Float => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        ColumnKind::Boolean => match text.to_ascii_uppercase().as_str() {
            "1" | "TRUE" | "T" => Some(Value::Bool(true)),
            "0" | "FALSE" | "F" => Some(Value::Bool(false)),
            _ => None,
        },
        ColumnKind::Blob | ColumnKind::Text => None,
    };
    typed.unwrap_or_else(|| Value::String(raw.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{cell_to_json, string_literal};
    use crate::client::ColumnKind;
    use serde_json::json;

    #[test]
    fn quotes_string_constants() {
        assert_eq!(string_literal("TEST_SCHEMA"), "'TEST_SCHEMA'");
        assert_eq!(string_literal("O'Brien"), "'O''Brien'");
        assert_eq!(string_literal(""), "''");
    }

    #[test]
    fn converts_cells_by_column_kind() {
        assert_eq!(cell_to_json(None, ColumnKind::Integer), json!(null));
        assert_eq!(cell_to_json(Some("42"), ColumnKind::Integer), json!(42));
        assert_eq!(
            cell_to_json(Some("-9223372036854775808"), ColumnKind::Integer),
            json!(i64::MIN)
        );
        assert_eq!(
            cell_to_json(Some("7.5E+004"), ColumnKind::Float),
            json!(75000.0)
        );
        assert_eq!(cell_to_json(Some("1"), ColumnKind::Boolean), json!(true));
        assert_eq!(
            cell_to_json(Some("FALSE"), ColumnKind::Boolean),
            json!(false)
        );
        // Text is passed through untouched, padding included.
        assert_eq!(
            cell_to_json(Some("abc  "), ColumnKind::Text),
            json!("abc  ")
        );
        // DECIMAL arrives as text and keeps every digit.
        assert_eq!(
            cell_to_json(Some("12345678901234567890.1234"), ColumnKind::Text),
            json!("12345678901234567890.1234")
        );
    }

    #[test]
    fn keeps_the_text_of_values_json_cannot_express() {
        assert_eq!(cell_to_json(Some("NaN"), ColumnKind::Float), json!("NaN"));
        assert_eq!(
            cell_to_json(Some("Infinity"), ColumnKind::Float),
            json!("Infinity")
        );
        assert_eq!(
            cell_to_json(Some("maybe"), ColumnKind::Boolean),
            json!("maybe")
        );
        assert_eq!(cell_to_json(Some("12x"), ColumnKind::Integer), json!("12x"));
    }
}
