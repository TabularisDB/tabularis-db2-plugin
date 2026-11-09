use crate::client::{Param, Session};
use crate::error::PluginResult;
use crate::utils::binding::{bind_value, ColumnSpec};
use crate::utils::blob::encode_blob_full;
use crate::utils::identifiers::{quote_identifier, quote_qualified_name};
use crate::utils::values::{json_to_sql_literal, string_literal};
use serde_json::Value;
use std::collections::HashMap;

/// Reads how each column of a table (or view) is written, in column order.
/// Empty when the catalog does not know the table.
pub fn column_specs(session: &Session, schema: &str, table: &str) -> PluginResult<Vec<ColumnSpec>> {
    let sql = format!(
        "SELECT COLNAME, TYPENAME, CODEPAGE FROM SYSCAT.COLUMNS \
         WHERE TABSCHEMA = {} AND TABNAME = {} ORDER BY COLNO WITH UR",
        string_literal(schema),
        string_literal(table)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .iter()
        .map(|row| {
            let cell = |index: usize| row.get(index).and_then(|value| value.as_deref());
            ColumnSpec::new(
                cell(0).unwrap_or_default(),
                cell(1).unwrap_or_default(),
                cell(2).and_then(|value| value.trim().parse::<i64>().ok()),
            )
        })
        .collect())
}

/// Names the column in Db2's "NULL not allowed" error.
///
/// SQL0407N identifies the column by internal numbers
/// (`"TBSPACEID=2, TABLEID=9, COLNO=0"`). `COLNO` is the column's position
/// in the table, which is the order `specs` is in, so the numbers are
/// replaced with the name a user can act on.
fn name_not_null_column(message: String, specs: &[ColumnSpec]) -> String {
    const MARKER: &str = "\"TBSPACEID=";
    let Some(start) = message.find(MARKER) else {
        return message;
    };
    let Some(length) = message[start + 1..].find('"') else {
        return message;
    };
    let end = start + 1 + length + 1;
    let column = message[start..end]
        .split("COLNO=")
        .nth(1)
        .and_then(|rest| rest.trim_end_matches('"').trim().parse::<usize>().ok())
        .and_then(|position| specs.get(position));
    match column {
        Some(column) => format!(
            "{}{}{}",
            &message[..start],
            quote_identifier(&column.name),
            &message[end..]
        ),
        None => message,
    }
}

fn spec_of<'a>(specs: &'a [ColumnSpec], column: &str) -> Option<&'a ColumnSpec> {
    specs.iter().find(|spec| spec.name == column)
}

/// Position of a column in the table, unknown columns last, so generated SQL
/// lists columns in table order whatever order the request used.
fn column_position(specs: &[ColumnSpec], column: &str) -> usize {
    specs
        .iter()
        .position(|spec| spec.name == column)
        .unwrap_or(usize::MAX)
}

/// Inserts one row and returns the number of rows inserted.
///
/// Each value is rendered for the type of its column (see
/// [`crate::utils::binding`]). A request with no values inserts a row made
/// of column defaults; Db2 has no `DEFAULT VALUES` clause, so the first
/// column is given `DEFAULT` explicitly.
pub fn insert_record(
    session: &Session,
    table: &str,
    data: &HashMap<String, Value>,
    schema: &str,
    max_blob_size: u64,
) -> PluginResult<u64> {
    let specs = column_specs(session, schema, table)?;
    let target = quote_qualified_name(Some(schema), table);

    let mut entries = data.iter().collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        column_position(&specs, a.0)
            .cmp(&column_position(&specs, b.0))
            .then_with(|| a.0.cmp(b.0))
    });

    if entries.is_empty() {
        let Some(first) = specs.first() else {
            return Err(format!("Table {target} was not found"));
        };
        let sql = format!(
            "INSERT INTO {target} ({}) VALUES (DEFAULT)",
            quote_identifier(&first.name)
        );
        return session
            .execute(&sql)
            .map_err(|error| name_not_null_column(error, &specs));
    }

    let mut params: Vec<Param> = Vec::new();
    let mut columns = Vec::with_capacity(entries.len());
    let mut values = Vec::with_capacity(entries.len());
    for (name, value) in entries {
        let bound = bind_value(value, spec_of(&specs, name), true, max_blob_size)?;
        columns.push(quote_identifier(name));
        values.push(bound.into_sql(&mut params));
    }

    let sql = format!(
        "INSERT INTO {target} ({}) VALUES ({})",
        columns.join(", "),
        values.join(", ")
    );
    session
        .execute_with(&sql, &params)
        .map_err(|error| name_not_null_column(error, &specs))
}

/// Builds the `WHERE` condition that identifies one row from the key values
/// Tabularis sends, pushing bound values to `params`.
///
/// `NULL` is matched with `IS NULL`. For a table without a primary key the
/// host sends every column of the row; the ones Db2 cannot compare with `=`
/// (LOBs, XML) are left out.
pub fn row_condition(
    specs: &[ColumnSpec],
    pk_map: &HashMap<String, Value>,
    target: &str,
    params: &mut Vec<Param>,
) -> PluginResult<String> {
    let mut keys = pk_map.iter().collect::<Vec<_>>();
    keys.sort_by(|a, b| {
        column_position(specs, a.0)
            .cmp(&column_position(specs, b.0))
            .then_with(|| a.0.cmp(b.0))
    });

    let mut conditions = Vec::with_capacity(keys.len());
    for (name, value) in keys {
        if name.is_empty() {
            continue;
        }
        let spec = spec_of(specs, name);
        if spec.is_some_and(|spec| !spec.is_comparable()) {
            continue;
        }
        let bound = bind_value(value, spec, false, u64::MAX)?;
        let column = quote_identifier(name);
        conditions.push(if bound.is_null() {
            format!("{column} IS NULL")
        } else {
            format!("{column} = {}", bound.into_sql(params))
        });
    }

    if conditions.is_empty() {
        return Err(format!(
            "Cannot identify the row in {target}: no key values were provided. \
             The table needs a primary key, or every column of the row must be loaded."
        ));
    }
    Ok(conditions.join(" AND "))
}

/// Reads the full value of a binary column for one row.
fn read_blob(
    session: &Session,
    table: &str,
    column: &str,
    pk_map: &HashMap<String, Value>,
    schema: &str,
) -> PluginResult<Vec<u8>> {
    let specs = column_specs(session, schema, table)?;
    let target = quote_qualified_name(Some(schema), table);
    let mut params = Vec::new();
    let condition = row_condition(&specs, pk_map, &target, &mut params)?;
    let sql = format!(
        "SELECT {} FROM {target} WHERE {condition} FETCH FIRST 1 ROW ONLY",
        quote_identifier(column)
    );
    session.fetch_bytes(&sql, &params)?.ok_or_else(|| {
        format!(
            "No value found in column {} of {target}",
            quote_identifier(column)
        )
    })
}

/// The whole value of a binary column, in the host's blob wire format, for
/// the preview in the row editor.
pub fn fetch_blob_as_data_url(
    session: &Session,
    table: &str,
    column: &str,
    pk_map: &HashMap<String, Value>,
    schema: &str,
) -> PluginResult<String> {
    read_blob(session, table, column, pk_map, schema).map(|bytes| encode_blob_full(&bytes))
}

/// Writes the whole value of a binary column to `file_path`.
pub fn save_blob_to_file(
    session: &Session,
    table: &str,
    column: &str,
    pk_map: &HashMap<String, Value>,
    schema: &str,
    file_path: &str,
) -> PluginResult<()> {
    let bytes = read_blob(session, table, column, pk_map, schema)?;
    std::fs::write(file_path, bytes)
        .map_err(|error| format!("Failed to write {file_path}: {error}"))
}

/// Sets one column of one row and returns the number of rows changed (1).
///
/// The row is found from the key values in `pk_map`: the primary key, or,
/// for a table without one, every column of the row as the grid loaded it.
/// The update goes through `UPDATE (SELECT ... FETCH FIRST 1 ROW ONLY)`, so
/// when a keyless table holds identical rows only one of them changes, as
/// the user edited one row. A key that matches nothing is an error rather
/// than a silent no-op: the row was changed or deleted since it was loaded.
pub fn update_record(
    session: &Session,
    table: &str,
    pk_map: &HashMap<String, Value>,
    column: &str,
    new_value: &Value,
    schema: &str,
    max_blob_size: u64,
) -> PluginResult<u64> {
    let specs = column_specs(session, schema, table)?;
    let target = quote_qualified_name(Some(schema), table);

    // The fullselect is written before the SET clause, so its parameters
    // come first.
    let mut params: Vec<Param> = Vec::new();
    let condition = row_condition(&specs, pk_map, &target, &mut params)?;
    let value =
        bind_value(new_value, spec_of(&specs, column), true, max_blob_size)?.into_sql(&mut params);

    let sql = format!(
        "UPDATE (SELECT * FROM {target} WHERE {condition} FETCH FIRST 1 ROW ONLY) SET {} = {value}",
        quote_identifier(column)
    );
    let affected = session
        .execute_with(&sql, &params)
        .map_err(|error| name_not_null_column(error, &specs))?;
    if affected == 0 {
        return Err(row_not_found(&target));
    }
    Ok(affected)
}

fn row_not_found(target: &str) -> String {
    format!(
        "No row in {target} matches the key values: it was changed or deleted after it was \
         loaded. Refresh the table and try again."
    )
}

pub fn delete_record(
    session: &Session,
    table: &str,
    pk_col: &str,
    pk_val: &Value,
    schema: Option<&str>,
) -> PluginResult<u64> {
    let sql = format!(
        "DELETE FROM {} WHERE {} = {}",
        quote_qualified_name(schema, table),
        quote_identifier(pk_col),
        json_to_sql_literal(pk_val)
    );
    session.execute(&sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn specs() -> Vec<ColumnSpec> {
        vec![
            ColumnSpec::new("ID", "INTEGER", Some(0)),
            ColumnSpec::new("First Name", "VARCHAR", Some(1208)),
            ColumnSpec::new("NOTES", "CLOB", Some(1208)),
            ColumnSpec::new("PRICE", "DECIMAL", Some(0)),
        ]
    }

    fn map(value: Value) -> HashMap<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn names_the_column_of_a_not_null_violation() {
        let message = "SQL0407N Assignment of a NULL value to a NOT NULL column \
                       \"TBSPACEID=2, TABLEID=9, COLNO=1\" is not allowed. SQLSTATE=23502";
        assert_eq!(
            name_not_null_column(message.to_string(), &specs()),
            "SQL0407N Assignment of a NULL value to a NOT NULL column \"First Name\" is not \
             allowed. SQLSTATE=23502"
        );
        // A position the table does not have, or another error, is left alone.
        let unknown = message.replace("COLNO=1", "COLNO=42");
        assert_eq!(name_not_null_column(unknown.clone(), &specs()), unknown);
        let other = "SQL0803N duplicate. SQLSTATE=23505".to_string();
        assert_eq!(name_not_null_column(other.clone(), &specs()), other);
    }

    #[test]
    fn builds_row_conditions_in_column_order() {
        let mut params = Vec::new();
        let condition = row_condition(
            &specs(),
            &map(json!({ "PRICE": "9.99", "First Name": "O'Brien", "ID": 7 })),
            "\"S\".\"T\"",
            &mut params,
        )
        .unwrap();
        assert_eq!(
            condition,
            "\"ID\" = 7 AND \"First Name\" = ? AND \"PRICE\" = 9.99"
        );
        assert_eq!(params, vec![Param::Text("O'Brien".to_string())]);
    }

    #[test]
    fn row_conditions_match_null_and_skip_what_db2_cannot_compare() {
        let mut params = Vec::new();
        let condition = row_condition(
            &specs(),
            &map(json!({ "ID": 7, "First Name": null, "NOTES": "long text" })),
            "\"S\".\"T\"",
            &mut params,
        )
        .unwrap();
        assert_eq!(condition, "\"ID\" = 7 AND \"First Name\" IS NULL");
        assert!(params.is_empty());
    }

    #[test]
    fn a_row_without_key_values_cannot_be_identified() {
        for keys in [
            json!({}),
            json!({ "": 1 }),
            json!({ "NOTES": "only a lob" }),
        ] {
            let error =
                row_condition(&specs(), &map(keys), "\"S\".\"T\"", &mut Vec::new()).unwrap_err();
            assert!(
                error.starts_with("Cannot identify the row in \"S\".\"T\""),
                "{error}"
            );
        }
    }

    #[test]
    fn key_values_are_validated_like_any_other() {
        let error = row_condition(
            &specs(),
            &map(json!({ "ID": "1 OR 1=1" })),
            "\"S\".\"T\"",
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(error.contains("is not a valid number"), "{error}");
    }
}
