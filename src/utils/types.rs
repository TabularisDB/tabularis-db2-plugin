use crate::models::ColumnDefinition;

pub fn column_type_sql(column: &ColumnDefinition) -> String {
    let mut sql = format!("{} {}", column.name, column.data_type);
    if !column.is_nullable {
        sql.push_str(" NOT NULL");
    }
    if let Some(default_value) = &column.default_value {
        sql.push_str(&format!(" DEFAULT {}", default_value));
    }
    sql
}

/// A column type as Tabularis wants it described.
#[derive(Debug, PartialEq, Eq)]
pub struct ColumnType {
    /// Base type name, the one listed in the manifest's `data_types`.
    pub data_type: String,
    /// Declared type with its modifiers, when it says more than `data_type`.
    pub full_type: Option<String>,
    pub character_maximum_length: Option<u64>,
    pub numeric_precision: Option<i32>,
    pub numeric_scale: Option<i32>,
}

/// Renders a LOB size the way Db2 DDL writes it (`1048576` -> `1M`).
fn lob_size(bytes: i64) -> String {
    const UNITS: [(i64, &str); 3] = [(1 << 30, "G"), (1 << 20, "M"), (1 << 10, "K")];
    for (unit, suffix) in UNITS {
        if bytes >= unit && bytes % unit == 0 {
            return format!("{}{suffix}", bytes / unit);
        }
    }
    bytes.to_string()
}

/// Describes a column type from its `SYSCAT.COLUMNS` row.
///
/// The catalog overloads `LENGTH` and `SCALE`: `LENGTH` is the declared
/// length for strings, the precision for `DECIMAL`, the storage size in
/// bytes for everything else (4 for `INTEGER` and `DATE`, 8 or 16 for
/// `DECFLOAT`), and `SCALE` is the fractional-seconds precision of a
/// `TIMESTAMP`. Reporting those as a "character maximum length" would make
/// Tabularis show and generate `INTEGER(4)`, so each family is mapped
/// explicitly. `CODEPAGE` 0 on a character type means `FOR BIT DATA`.
pub fn describe_catalog_type(
    type_name: &str,
    length: Option<i64>,
    scale: Option<i64>,
    codepage: Option<i64>,
) -> ColumnType {
    let type_name = type_name.trim();
    let upper = type_name.to_uppercase();
    // The catalog says CHARACTER; DDL and the manifest's type list say CHAR.
    let data_type = if upper == "CHARACTER" {
        "CHAR".to_string()
    } else {
        type_name.to_string()
    };

    let mut column_type = ColumnType {
        data_type: data_type.clone(),
        full_type: None,
        character_maximum_length: None,
        numeric_precision: None,
        numeric_scale: None,
    };

    let Some(length) = length.filter(|value| *value >= 0) else {
        return column_type;
    };

    match upper.as_str() {
        "CHARACTER" | "VARCHAR" | "LONG VARCHAR" => {
            column_type.character_maximum_length = Some(length as u64);
            let bit_data = if codepage == Some(0) {
                " FOR BIT DATA"
            } else {
                ""
            };
            column_type.full_type = Some(if upper == "LONG VARCHAR" {
                format!("{data_type}{bit_data}")
            } else {
                format!("{data_type}({length}){bit_data}")
            })
            .filter(|full_type| *full_type != data_type);
        }
        "GRAPHIC" | "VARGRAPHIC" | "BINARY" | "VARBINARY" => {
            column_type.character_maximum_length = Some(length as u64);
            column_type.full_type = Some(format!("{data_type}({length})"));
        }
        "LONG VARGRAPHIC" => {
            column_type.character_maximum_length = Some(length as u64);
        }
        "CLOB" | "DBCLOB" | "BLOB" => {
            column_type.character_maximum_length = Some(length as u64);
            column_type.full_type = Some(format!("{data_type}({})", lob_size(length)));
        }
        "DECIMAL" => {
            let scale = scale.unwrap_or(0);
            column_type.numeric_precision = Some(length as i32);
            column_type.numeric_scale = Some(scale as i32);
            column_type.full_type = Some(format!("{data_type}({length},{scale})"));
        }
        "DECFLOAT" => {
            // Stored in 8 or 16 bytes: DECFLOAT(16) or DECFLOAT(34).
            let precision = if length <= 8 { 16 } else { 34 };
            column_type.numeric_precision = Some(precision);
            column_type.full_type = Some(format!("{data_type}({precision})"));
        }
        "TIMESTAMP" => {
            // 6 is the default precision; only a non-default one is worth showing.
            if let Some(precision) = scale.filter(|value| *value != 6) {
                column_type.full_type = Some(format!("{data_type}({precision})"));
            }
        }
        _ => {}
    }

    column_type
}

#[cfg(test)]
mod tests {
    use super::{column_type_sql, describe_catalog_type, lob_size};
    use crate::models::ColumnDefinition;

    #[test]
    fn builds_column_definition_sql() {
        let column = ColumnDefinition {
            name: "ID".to_string(),
            data_type: "INTEGER".to_string(),
            is_nullable: false,
            is_pk: true,
            is_auto_increment: false,
            default_value: None,
        };
        assert_eq!(column_type_sql(&column), "ID INTEGER NOT NULL");
    }

    #[test]
    fn storage_sizes_are_not_reported_as_lengths() {
        for (type_name, bytes) in [
            ("SMALLINT", 2),
            ("INTEGER", 4),
            ("BIGINT", 8),
            ("REAL", 4),
            ("DOUBLE", 8),
            ("DATE", 4),
            ("TIME", 3),
            ("BOOLEAN", 1),
            ("XML", 0),
        ] {
            let described = describe_catalog_type(type_name, Some(bytes), Some(0), Some(0));
            assert_eq!(described.data_type, type_name);
            assert_eq!(described.character_maximum_length, None, "{type_name}");
            assert_eq!(described.numeric_precision, None, "{type_name}");
            assert_eq!(described.full_type, None, "{type_name}");
        }
    }

    #[test]
    fn describes_character_types() {
        let varchar = describe_catalog_type("VARCHAR", Some(50), Some(0), Some(1208));
        assert_eq!(varchar.data_type, "VARCHAR");
        assert_eq!(varchar.character_maximum_length, Some(50));
        assert_eq!(varchar.full_type.as_deref(), Some("VARCHAR(50)"));

        // The catalog name CHARACTER is reported as the DDL name CHAR.
        let character = describe_catalog_type("CHARACTER", Some(10), Some(0), Some(1208));
        assert_eq!(character.data_type, "CHAR");
        assert_eq!(character.full_type.as_deref(), Some("CHAR(10)"));

        let bit_data = describe_catalog_type("VARCHAR", Some(16), Some(0), Some(0));
        assert_eq!(
            bit_data.full_type.as_deref(),
            Some("VARCHAR(16) FOR BIT DATA")
        );

        let graphic = describe_catalog_type("VARGRAPHIC", Some(255), Some(0), Some(1200));
        assert_eq!(graphic.full_type.as_deref(), Some("VARGRAPHIC(255)"));
        assert_eq!(graphic.character_maximum_length, Some(255));
    }

    #[test]
    fn describes_numeric_and_temporal_types() {
        let decimal = describe_catalog_type("DECIMAL", Some(10), Some(2), Some(0));
        assert_eq!(decimal.numeric_precision, Some(10));
        assert_eq!(decimal.numeric_scale, Some(2));
        assert_eq!(decimal.character_maximum_length, None);
        assert_eq!(decimal.full_type.as_deref(), Some("DECIMAL(10,2)"));

        let decfloat = describe_catalog_type("DECFLOAT", Some(16), Some(0), Some(0));
        assert_eq!(decfloat.numeric_precision, Some(34));
        assert_eq!(decfloat.full_type.as_deref(), Some("DECFLOAT(34)"));
        let decfloat = describe_catalog_type("DECFLOAT", Some(8), Some(0), Some(0));
        assert_eq!(decfloat.full_type.as_deref(), Some("DECFLOAT(16)"));

        let timestamp = describe_catalog_type("TIMESTAMP", Some(10), Some(6), Some(0));
        assert_eq!(timestamp.full_type, None);
        let timestamp = describe_catalog_type("TIMESTAMP", Some(13), Some(12), Some(0));
        assert_eq!(timestamp.full_type.as_deref(), Some("TIMESTAMP(12)"));
    }

    #[test]
    fn describes_large_objects() {
        let clob = describe_catalog_type("CLOB", Some(1_048_576), Some(0), Some(1208));
        assert_eq!(clob.character_maximum_length, Some(1_048_576));
        assert_eq!(clob.full_type.as_deref(), Some("CLOB(1M)"));

        assert_eq!(lob_size(2_147_483_647), "2147483647");
        assert_eq!(lob_size(1 << 30), "1G");
        assert_eq!(lob_size(4096), "4K");
        assert_eq!(lob_size(100), "100");
    }

    #[test]
    fn tolerates_missing_catalog_values() {
        let described = describe_catalog_type("VARCHAR", None, None, None);
        assert_eq!(described.data_type, "VARCHAR");
        assert_eq!(described.full_type, None);
        assert_eq!(described.character_maximum_length, None);
    }
}
