//! Turns the JSON values Tabularis sends for a row into SQL for Db2.
//!
//! The grid sends what the user typed, mostly as strings, whatever the
//! column type. Each value is rendered for the type of the column it goes
//! into: numbers and booleans as validated literals, text and binary data as
//! bound parameters, and the host's special forms (`NULL`, "use the default",
//! the BLOB wire formats) as what they stand for.

use crate::client::Param;
use crate::error::PluginResult;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;

/// What Tabularis sends for "use the column default".
pub const USE_DEFAULT_SENTINEL: &str = "__USE_DEFAULT__";

/// Largest BLOB accepted when the request does not say (100 MiB, the host's
/// own default).
pub const DEFAULT_MAX_BLOB_SIZE: u64 = 100 * 1024 * 1024;

/// How values of a column are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFamily {
    /// `SMALLINT`, `INTEGER`, `BIGINT`, `DECIMAL`, `REAL`, `DOUBLE`,
    /// `DECFLOAT`: a numeric literal.
    Numeric,
    /// `BOOLEAN`: `TRUE` / `FALSE`.
    Boolean,
    /// `DATE`, `TIME`, `TIMESTAMP`: text Db2 converts on assignment.
    Temporal,
    /// `BLOB`, `BINARY`, `VARBINARY` and `FOR BIT DATA` strings: bytes.
    Binary,
    /// `CLOB`, `DBCLOB`, `XML`, `LONG VARCHAR`: text, but not comparable
    /// with `=`.
    LargeText,
    /// Everything else: text.
    Text,
}

/// The part of a column's catalog entry that decides how to write to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub family: ColumnFamily,
}

impl ColumnSpec {
    /// Classifies a column from `SYSCAT.COLUMNS.TYPENAME` and `CODEPAGE`
    /// (0 on a character type means `FOR BIT DATA`).
    pub fn new(name: &str, type_name: &str, codepage: Option<i64>) -> Self {
        let type_name = type_name.trim().to_uppercase();
        let family = match type_name.as_str() {
            "SMALLINT" | "INTEGER" | "BIGINT" | "DECIMAL" | "REAL" | "DOUBLE" | "DECFLOAT" => {
                ColumnFamily::Numeric
            }
            "BOOLEAN" => ColumnFamily::Boolean,
            "DATE" | "TIME" | "TIMESTAMP" => ColumnFamily::Temporal,
            "BLOB" | "BINARY" | "VARBINARY" => ColumnFamily::Binary,
            "CHARACTER" | "VARCHAR" if codepage == Some(0) => ColumnFamily::Binary,
            "LONG VARCHAR" if codepage == Some(0) => ColumnFamily::Binary,
            "CLOB" | "DBCLOB" | "XML" | "LONG VARCHAR" | "LONG VARGRAPHIC" => {
                ColumnFamily::LargeText
            }
            _ => ColumnFamily::Text,
        };
        Self {
            name: name.to_string(),
            family,
        }
    }

    /// Whether a value of this column can be matched with `=` in a `WHERE`
    /// clause. Db2 does not compare LOB, XML and LONG columns.
    pub fn is_comparable(&self) -> bool {
        self.family != ColumnFamily::LargeText
    }
}

/// A value ready to be placed in a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlValue {
    /// SQL text placed in the statement as is: `NULL`, `DEFAULT`, `42`,
    /// `TRUE`.
    Literal(String),
    /// A `?` marker with the value bound to it.
    Bound(Param),
}

impl SqlValue {
    pub fn null() -> Self {
        Self::Literal("NULL".to_string())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Literal(text) if text == "NULL")
    }

    /// The SQL fragment for this value; a bound value is pushed to `params`
    /// and stands as `?`.
    pub fn into_sql(self, params: &mut Vec<Param>) -> String {
        match self {
            Self::Literal(text) => text,
            Self::Bound(param) => {
                params.push(param);
                "?".to_string()
            }
        }
    }
}

/// Renders `value` for `column`.
///
/// `column` is `None` when the catalog does not know the column (the
/// statement will then fail in Db2 with its own message); the value is then
/// rendered from its JSON type alone. `allow_default` is true where
/// `DEFAULT` is valid SQL: the values of an `INSERT` and the right-hand side
/// of an `UPDATE ... SET`.
pub fn bind_value(
    value: &Value,
    column: Option<&ColumnSpec>,
    allow_default: bool,
    max_blob_size: u64,
) -> PluginResult<SqlValue> {
    if value.is_null() {
        return Ok(SqlValue::null());
    }
    if value.as_str() == Some(USE_DEFAULT_SENTINEL) {
        return if allow_default {
            Ok(SqlValue::Literal("DEFAULT".to_string()))
        } else {
            Err("A column default cannot be used to identify a row".to_string())
        };
    }

    let Some(column) = column else {
        return Ok(match value {
            Value::Number(number) => SqlValue::Literal(number.to_string()),
            Value::Bool(flag) => SqlValue::Literal(if *flag { "1" } else { "0" }.to_string()),
            other => SqlValue::Bound(Param::Text(json_text(other))),
        });
    };

    match column.family {
        ColumnFamily::Numeric => bind_numeric(value, column),
        ColumnFamily::Boolean => bind_boolean(value, column),
        ColumnFamily::Temporal => Ok(bind_temporal(value)),
        ColumnFamily::Binary => bind_binary(value, max_blob_size),
        ColumnFamily::LargeText | ColumnFamily::Text => {
            Ok(SqlValue::Bound(Param::Text(json_text(value))))
        }
    }
}

/// The text of a JSON value: a string as it is, anything else as JSON.
fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// True for the spellings Db2 accepts as a numeric constant: an optional
/// sign, digits with an optional decimal point, an optional exponent, plus
/// the `DECFLOAT` special values.
fn is_numeric_constant(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if matches!(
        unsigned.to_ascii_uppercase().as_str(),
        "NAN" | "SNAN" | "INFINITY" | "INF"
    ) {
        return true;
    }

    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    mantissa_ok && exponent_ok
}

fn bind_numeric(value: &Value, column: &ColumnSpec) -> PluginResult<SqlValue> {
    match value {
        Value::Number(number) => Ok(SqlValue::Literal(number.to_string())),
        Value::Bool(flag) => Ok(SqlValue::Literal(if *flag { "1" } else { "0" }.to_string())),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                // A cleared numeric cell means "no value".
                return Ok(SqlValue::null());
            }
            if !is_numeric_constant(text) {
                return Err(format!(
                    "'{text}' is not a valid number for column \"{}\"",
                    column.name
                ));
            }
            let special = text.trim_start_matches(['+', '-']).to_ascii_uppercase();
            if matches!(special.as_str(), "NAN" | "SNAN" | "INFINITY" | "INF") {
                // Only DECFLOAT has these; they are written as a string cast.
                return Ok(SqlValue::Literal(format!("DECFLOAT('{text}')")));
            }
            Ok(SqlValue::Literal(text.to_string()))
        }
        other => Err(format!(
            "{other} is not a valid number for column \"{}\"",
            column.name
        )),
    }
}

fn bind_boolean(value: &Value, column: &ColumnSpec) -> PluginResult<SqlValue> {
    let flag = match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(SqlValue::null()),
            "true" | "t" | "yes" | "y" | "on" | "1" => true,
            "false" | "f" | "no" | "n" | "off" | "0" => false,
            other => {
                return Err(format!(
                    "'{other}' is not a valid boolean for column \"{}\"",
                    column.name
                ))
            }
        },
        other => {
            return Err(format!(
                "{other} is not a valid boolean for column \"{}\"",
                column.name
            ))
        }
    };
    Ok(SqlValue::Literal(
        if flag { "TRUE" } else { "FALSE" }.to_string(),
    ))
}

/// Dates and times are bound as text and converted by Db2, which reports a
/// malformed one itself. The ISO `T` separator and a trailing `Z`, which
/// date pickers produce and Db2 does not read, are normalized first.
fn bind_temporal(value: &Value) -> SqlValue {
    let text = json_text(value);
    let text = text.trim();
    if text.is_empty() {
        return SqlValue::null();
    }
    let mut normalized = text.strip_suffix(['Z', 'z']).unwrap_or(text).to_string();
    if normalized.as_bytes().get(10) == Some(&b'T') {
        normalized.replace_range(10..11, " ");
    }
    SqlValue::Bound(Param::Text(normalized))
}

/// Decodes text made only of hexadecimal digit pairs, the form `BINARY`,
/// `VARBINARY` and `FOR BIT DATA` values are shown in.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

/// Splits `<size>:<mime>:<rest>`, the layout shared by the blob wire forms.
fn blob_fields(payload: &str) -> Option<(u64, &str)> {
    let (size, rest) = payload.split_once(':')?;
    let (_mime, rest) = rest.split_once(':')?;
    Some((size.parse().ok()?, rest))
}

fn bind_binary(value: &Value, max_blob_size: u64) -> PluginResult<SqlValue> {
    let text = json_text(value);

    // A file picked in the UI: the host hands over a path to read.
    if let Some(payload) = text.strip_prefix("BLOB_FILE_REF:") {
        let (size, path) =
            blob_fields(payload).ok_or_else(|| "Invalid BLOB file reference".to_string())?;
        if size > max_blob_size {
            return Err(format!(
                "File size ({size} bytes) exceeds the maximum allowed BLOB size \
                 ({max_blob_size} bytes)"
            ));
        }
        let bytes =
            std::fs::read(path).map_err(|error| format!("Failed to read BLOB file: {error}"))?;
        if bytes.len() as u64 > max_blob_size {
            return Err(format!(
                "File size ({} bytes) exceeds the maximum allowed BLOB size ({max_blob_size} bytes)",
                bytes.len()
            ));
        }
        return Ok(SqlValue::Bound(Param::Bytes(bytes)));
    }

    // Inline bytes in the host's wire format.
    if let Some(payload) = text.strip_prefix("BLOB:") {
        if let Some((size, base64_data)) = blob_fields(payload) {
            if let Ok(bytes) = STANDARD.decode(base64_data) {
                if (bytes.len() as u64) < size {
                    // The grid only holds a preview of a large value; writing
                    // it back would cut the stored value down to it.
                    return Err(format!(
                        "The value holds {} of {size} bytes: it is a preview and cannot be \
                         written back. Upload the file again to replace the value.",
                        bytes.len()
                    ));
                }
                if bytes.len() as u64 > max_blob_size {
                    return Err(format!(
                        "BLOB size ({} bytes) exceeds the maximum allowed size ({max_blob_size} bytes)",
                        bytes.len()
                    ));
                }
                return Ok(SqlValue::Bound(Param::Bytes(bytes)));
            }
        }
    }

    // Hexadecimal text is how these columns are displayed, so it is read
    // back as the bytes it shows. Anything else is stored as its UTF-8 bytes.
    let bytes = decode_hex(text.trim()).unwrap_or_else(|| text.into_bytes());
    Ok(SqlValue::Bound(Param::Bytes(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MAX: u64 = DEFAULT_MAX_BLOB_SIZE;

    fn column(type_name: &str) -> ColumnSpec {
        ColumnSpec::new("C", type_name, Some(1208))
    }

    fn bind(value: Value, type_name: &str) -> PluginResult<SqlValue> {
        bind_value(&value, Some(&column(type_name)), true, MAX)
    }

    fn literal(text: &str) -> SqlValue {
        SqlValue::Literal(text.to_string())
    }

    fn text(value: &str) -> SqlValue {
        SqlValue::Bound(Param::Text(value.to_string()))
    }

    fn bytes(value: &[u8]) -> SqlValue {
        SqlValue::Bound(Param::Bytes(value.to_vec()))
    }

    #[test]
    fn classifies_columns() {
        for (type_name, family) in [
            ("INTEGER", ColumnFamily::Numeric),
            ("DECIMAL", ColumnFamily::Numeric),
            ("DECFLOAT", ColumnFamily::Numeric),
            ("BOOLEAN", ColumnFamily::Boolean),
            ("TIMESTAMP", ColumnFamily::Temporal),
            ("BLOB", ColumnFamily::Binary),
            ("VARBINARY", ColumnFamily::Binary),
            ("CLOB", ColumnFamily::LargeText),
            ("XML", ColumnFamily::LargeText),
            ("VARCHAR", ColumnFamily::Text),
            ("CHARACTER", ColumnFamily::Text),
            ("VARGRAPHIC", ColumnFamily::Text),
            ("MY_DISTINCT_TYPE", ColumnFamily::Text),
        ] {
            assert_eq!(column(type_name).family, family, "{type_name}");
        }
        // FOR BIT DATA: a character type holding bytes.
        assert_eq!(
            ColumnSpec::new("C", "VARCHAR", Some(0)).family,
            ColumnFamily::Binary
        );
        assert!(column("VARCHAR").is_comparable());
        assert!(!column("CLOB").is_comparable());
        assert!(!column("XML").is_comparable());
    }

    #[test]
    fn null_and_default_do_not_depend_on_the_type() {
        for type_name in ["INTEGER", "VARCHAR", "BLOB", "DATE", "BOOLEAN"] {
            assert_eq!(bind(json!(null), type_name), Ok(literal("NULL")));
            assert_eq!(
                bind(json!(USE_DEFAULT_SENTINEL), type_name),
                Ok(literal("DEFAULT"))
            );
        }
        // Not where a value is compared.
        assert!(bind_value(
            &json!(USE_DEFAULT_SENTINEL),
            Some(&column("INTEGER")),
            false,
            MAX
        )
        .is_err());
    }

    #[test]
    fn numbers_are_validated_literals() {
        assert_eq!(bind(json!(42), "INTEGER"), Ok(literal("42")));
        assert_eq!(bind(json!(-1.5), "DOUBLE"), Ok(literal("-1.5")));
        assert_eq!(bind(json!("12.50"), "DECIMAL"), Ok(literal("12.50")));
        assert_eq!(bind(json!("  -7 "), "SMALLINT"), Ok(literal("-7")));
        assert_eq!(bind(json!("1.5E+3"), "DOUBLE"), Ok(literal("1.5E+3")));
        assert_eq!(bind(json!(".5"), "DECIMAL"), Ok(literal(".5")));
        assert_eq!(
            bind(json!("12345678901234567890.123456789"), "DECFLOAT"),
            Ok(literal("12345678901234567890.123456789"))
        );
        assert_eq!(
            bind(json!("NaN"), "DECFLOAT"),
            Ok(literal("DECFLOAT('NaN')"))
        );
        assert_eq!(bind(json!(true), "INTEGER"), Ok(literal("1")));
        // A cleared cell is NULL, not 0.
        assert_eq!(bind(json!(""), "INTEGER"), Ok(literal("NULL")));
        assert_eq!(bind(json!("   "), "DECIMAL"), Ok(literal("NULL")));
    }

    #[test]
    fn text_that_is_not_a_number_never_reaches_the_statement() {
        for bad in [
            "abc",
            "1; DROP TABLE T",
            "1 OR 1=1",
            "1e",
            "--1",
            "1.2.3",
            "0x10",
            ".",
        ] {
            let error = bind(json!(bad), "INTEGER").unwrap_err();
            assert!(error.contains("is not a valid number"), "{bad}: {error}");
            assert!(error.contains("\"C\""), "{error}");
        }
        assert!(bind(json!([1]), "INTEGER").is_err());
    }

    #[test]
    fn booleans_accept_the_usual_spellings() {
        assert_eq!(bind(json!(true), "BOOLEAN"), Ok(literal("TRUE")));
        assert_eq!(bind(json!(false), "BOOLEAN"), Ok(literal("FALSE")));
        assert_eq!(bind(json!(1), "BOOLEAN"), Ok(literal("TRUE")));
        assert_eq!(bind(json!(0), "BOOLEAN"), Ok(literal("FALSE")));
        assert_eq!(bind(json!("true"), "BOOLEAN"), Ok(literal("TRUE")));
        assert_eq!(bind(json!("F"), "BOOLEAN"), Ok(literal("FALSE")));
        assert_eq!(bind(json!(""), "BOOLEAN"), Ok(literal("NULL")));
        assert!(bind(json!("maybe"), "BOOLEAN").is_err());
    }

    #[test]
    fn text_is_bound_never_inlined() {
        assert_eq!(bind(json!("O'Brien"), "VARCHAR"), Ok(text("O'Brien")));
        assert_eq!(
            bind(json!("'; DROP TABLE T; --"), "VARCHAR"),
            Ok(text("'; DROP TABLE T; --"))
        );
        // An empty string is an empty string for text.
        assert_eq!(bind(json!(""), "VARCHAR"), Ok(text("")));
        assert_eq!(bind(json!("<a/>"), "XML"), Ok(text("<a/>")));
        assert_eq!(bind(json!(12), "VARCHAR"), Ok(text("12")));
        assert_eq!(bind(json!({"a": 1}), "CLOB"), Ok(text("{\"a\":1}")));
    }

    #[test]
    fn dates_and_times_are_bound_text() {
        assert_eq!(bind(json!("2024-02-29"), "DATE"), Ok(text("2024-02-29")));
        assert_eq!(bind(json!("13:45:10"), "TIME"), Ok(text("13:45:10")));
        assert_eq!(
            bind(json!("2024-02-29T13:45:10.123Z"), "TIMESTAMP"),
            Ok(text("2024-02-29 13:45:10.123"))
        );
        assert_eq!(
            bind(json!("2024-02-29 13:45:10.123456"), "TIMESTAMP"),
            Ok(text("2024-02-29 13:45:10.123456"))
        );
        assert_eq!(bind(json!(""), "DATE"), Ok(literal("NULL")));
    }

    #[test]
    fn binary_values_are_bound_bytes() {
        // The host's inline wire format.
        assert_eq!(
            bind(json!("BLOB:4:application/octet-stream:yv66vg=="), "BLOB"),
            Ok(bytes(&[0xCA, 0xFE, 0xBA, 0xBE]))
        );
        assert_eq!(
            bind(json!("BLOB:0:application/octet-stream:"), "BLOB"),
            Ok(bytes(&[]))
        );
        // Hex text, as BINARY / VARBINARY / FOR BIT DATA values are shown.
        assert_eq!(
            bind(json!("DEADBEEF"), "VARBINARY"),
            Ok(bytes(&[0xDE, 0xAD, 0xBE, 0xEF]))
        );
        assert_eq!(bind(json!("cafe"), "BINARY"), Ok(bytes(&[0xCA, 0xFE])));
        // Anything else is stored as its UTF-8 bytes.
        assert_eq!(bind(json!("hello"), "BLOB"), Ok(bytes(b"hello")));
        assert_eq!(bind(json!("abc"), "VARBINARY"), Ok(bytes(b"abc")));
        // FOR BIT DATA is binary too.
        assert_eq!(
            bind_value(
                &json!("CAFEBABE"),
                Some(&ColumnSpec::new("C", "VARCHAR", Some(0))),
                true,
                MAX
            ),
            Ok(bytes(&[0xCA, 0xFE, 0xBA, 0xBE]))
        );
    }

    #[test]
    fn a_blob_preview_is_not_written_back() {
        // Header says 50,000 bytes, payload holds 4: a truncated preview.
        let error = bind(
            json!("BLOB:50000:application/octet-stream:yv66vg=="),
            "BLOB",
        )
        .unwrap_err();
        assert!(error.contains("preview"), "{error}");
    }

    #[test]
    fn blobs_respect_the_size_limit() {
        let wire = format!(
            "BLOB:4:application/octet-stream:{}",
            STANDARD.encode([1, 2, 3, 4])
        );
        let error = bind_value(&json!(wire), Some(&column("BLOB")), true, 3).unwrap_err();
        assert!(error.contains("exceeds the maximum"), "{error}");

        let error = bind_value(
            &json!("BLOB_FILE_REF:4096:image/png:/no/such/file"),
            Some(&column("BLOB")),
            true,
            1024,
        )
        .unwrap_err();
        assert!(error.contains("exceeds the maximum"), "{error}");
    }

    #[test]
    fn a_blob_file_reference_is_read_from_disk() {
        let path = std::env::temp_dir().join(format!("db2-plugin-blob-{}.bin", std::process::id()));
        std::fs::write(&path, [9u8, 8, 7]).unwrap();
        let reference = format!(
            "BLOB_FILE_REF:3:application/octet-stream:{}",
            path.display()
        );
        let bound = bind(json!(reference), "BLOB");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bound, Ok(bytes(&[9, 8, 7])));

        let error = bind(
            json!("BLOB_FILE_REF:3:application/octet-stream:/no/such/file"),
            "BLOB",
        )
        .unwrap_err();
        assert!(error.starts_with("Failed to read BLOB file"), "{error}");
    }

    #[test]
    fn an_unknown_column_is_rendered_from_the_json_type() {
        assert_eq!(bind_value(&json!(5), None, true, MAX), Ok(literal("5")));
        assert_eq!(bind_value(&json!("5"), None, true, MAX), Ok(text("5")));
        assert_eq!(bind_value(&json!(true), None, true, MAX), Ok(literal("1")));
        assert_eq!(
            bind_value(&json!(null), None, true, MAX),
            Ok(literal("NULL"))
        );
    }

    #[test]
    fn bound_values_become_markers_in_order() {
        let mut params = Vec::new();
        let sql = [literal("NULL"), text("a"), literal("7"), bytes(&[1])]
            .into_iter()
            .map(|value| value.into_sql(&mut params))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(sql, "NULL, ?, 7, ?");
        assert_eq!(
            params,
            vec![Param::Text("a".to_string()), Param::Bytes(vec![1])]
        );
    }
}
