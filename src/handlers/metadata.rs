use crate::client::Session;
use crate::error::PluginResult;
use crate::models::{
    ForeignKey, Index, RoutineInfo, RoutineParameter, TableColumn, TableInfo, TableSchema, ViewInfo,
};
use crate::utils::identifiers::{quote_identifier, quote_qualified_name};
use crate::utils::types::describe_catalog_type;
use crate::utils::values::string_literal;
use crate::utils::views::{is_create_view_statement, parse_view_text, view_query};
use std::collections::HashMap;

fn cell(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index).and_then(|value| value.as_deref())
}

fn cell_i64(row: &[Option<String>], index: usize) -> Option<i64> {
    cell(row, index).and_then(|value| value.trim().parse::<i64>().ok())
}

/// Catalog text columns hold `NULL` or an empty string for "nothing".
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

pub fn get_databases(session: &Session) -> PluginResult<Vec<String>> {
    let result = session.query("SELECT CURRENT SERVER AS NAME FROM SYSIBM.SYSDUMMY1 WITH UR")?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

/// Lists the schemas a user can browse: every schema in the catalog, empty
/// ones included, minus the ones Db2 owns (`SYS*`, plus `NULLID` and `SQLJ`,
/// which hold the driver packages and the JDBC/SQLJ routines).
pub fn get_schemas(session: &Session) -> PluginResult<Vec<String>> {
    let result = session.query(
        "SELECT SCHEMANAME FROM SYSCAT.SCHEMATA \
         WHERE SCHEMANAME NOT LIKE 'SYS%' AND SCHEMANAME NOT IN ('NULLID', 'SQLJ') \
         ORDER BY SCHEMANAME WITH UR",
    )?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

/// Lists the tables of a schema: base tables (`T`), typed tables (`U`),
/// materialized query tables (`S`), created temporary tables (`G`) and
/// nicknames (`N`). All of them have their columns in `SYSCAT.COLUMNS` and
/// can be queried like a table. Views are listed by `get_views`; aliases are
/// left out because the catalog describes no columns for them.
pub fn get_tables(session: &Session, schema: &str) -> PluginResult<Vec<TableInfo>> {
    let sql = format!(
        "SELECT TABNAME, REMARKS FROM SYSCAT.TABLES \
         WHERE TABSCHEMA = {} AND TYPE IN ('T', 'U', 'S', 'G', 'N') \
         ORDER BY TABNAME WITH UR",
        string_literal(schema)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| {
            cell(&row, 0).map(|name| TableInfo {
                name: name.to_string(),
                comment: non_empty(cell(&row, 1)),
            })
        })
        .collect())
}

/// Lists the views of a schema with their query.
pub fn get_views(session: &Session, schema: &str) -> PluginResult<Vec<ViewInfo>> {
    let sql = format!(
        "SELECT VIEWNAME, TEXT FROM SYSCAT.VIEWS WHERE VIEWSCHEMA = {} ORDER BY VIEWNAME WITH UR",
        string_literal(schema)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| {
            cell(&row, 0).map(|name| ViewInfo {
                name: name.to_string(),
                definition: cell(&row, 1).map(|text| view_query(text).to_string()),
            })
        })
        .collect())
}

/// The statement Db2 stored for a view, as it was typed.
fn view_text(session: &Session, schema: &str, view_name: &str) -> PluginResult<Option<String>> {
    let sql = format!(
        "SELECT TEXT FROM SYSCAT.VIEWS WHERE VIEWSCHEMA = {} AND VIEWNAME = {} WITH UR",
        string_literal(schema),
        string_literal(view_name)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next().flatten()))
}

/// The query of a view: what follows `AS` in its `CREATE VIEW`. This is the
/// form Tabularis edits and hands back to `alter_view`.
pub fn get_view_definition(
    session: &Session,
    schema: &str,
    view_name: &str,
) -> PluginResult<String> {
    let target = quote_qualified_name(Some(schema), view_name);
    let text = view_text(session, schema, view_name)?
        .ok_or_else(|| format!("View {target} was not found"))?;
    Ok(view_query(&text).to_string())
}

/// A definition as Db2 takes it after `AS`: no surrounding blank space and
/// no statement terminator.
fn view_body(definition: &str) -> PluginResult<&str> {
    let body = definition.trim().trim_end_matches(';').trim_end();
    if body.is_empty() {
        return Err("A view needs a query".to_string());
    }
    Ok(body)
}

/// Creates a view from its query. A definition that is already a complete
/// `CREATE VIEW` statement is run as written.
pub fn create_view(
    session: &Session,
    schema: &str,
    view_name: &str,
    definition: &str,
) -> PluginResult<()> {
    let body = view_body(definition)?;
    let sql = if is_create_view_statement(body) {
        body.to_string()
    } else {
        format!(
            "CREATE VIEW {} AS {body}",
            quote_qualified_name(Some(schema), view_name)
        )
    };
    session.execute(&sql).map(|_| ())
}

/// Replaces the query of an existing view, keeping its privileges
/// (`CREATE OR REPLACE VIEW`).
///
/// A view declared with an explicit column list (`CREATE VIEW V (A, B) AS
/// ...`) keeps it: Tabularis only edits the query, and replacing the view
/// without the list would rename its columns to whatever the query calls
/// them.
pub fn alter_view(
    session: &Session,
    schema: &str,
    view_name: &str,
    definition: &str,
) -> PluginResult<()> {
    let body = view_body(definition)?;
    let target = quote_qualified_name(Some(schema), view_name);
    let existing = view_text(session, schema, view_name)?
        .ok_or_else(|| format!("View {target} was not found"))?;

    let sql = if is_create_view_statement(body) {
        body.to_string()
    } else {
        let column_list = parse_view_text(&existing)
            .and_then(|view| view.column_list)
            .map(|list| format!(" {list}"))
            .unwrap_or_default();
        format!("CREATE OR REPLACE VIEW {target}{column_list} AS {body}")
    };
    session.execute(&sql).map(|_| ())
}

pub fn drop_view(session: &Session, schema: &str, view_name: &str) -> PluginResult<()> {
    let sql = format!(
        "DROP VIEW {}",
        quote_qualified_name(Some(schema), view_name)
    );
    session.execute(&sql).map(|_| ())
}

/// Column list shared by `get_columns` and `get_all_columns_batch`; the
/// table name comes first so the batch can group on it.
///
/// Implicitly hidden columns are skipped: `SELECT *` does not return them,
/// and Tabularis expects the metadata to describe the same columns the grid
/// shows.
const COLUMN_SELECT: &str =
    "SELECT TABNAME, COLNAME, TYPENAME, LENGTH, SCALE, CODEPAGE, KEYSEQ, NULLS, IDENTITY, \
            GENERATED, DEFAULT, REMARKS \
     FROM SYSCAT.COLUMNS";

fn column_from_row(row: &[Option<String>]) -> TableColumn {
    let column_type = describe_catalog_type(
        cell(row, 2).unwrap_or_default(),
        cell_i64(row, 3),
        cell_i64(row, 4),
        cell_i64(row, 5),
    );
    let is_identity = matches!(cell(row, 8), Some("Y"));
    let default_value = non_empty(cell(row, 10));

    TableColumn {
        name: cell(row, 1).unwrap_or_default().to_string(),
        data_type: column_type.data_type,
        is_pk: cell_i64(row, 6).unwrap_or(0) > 0,
        is_nullable: matches!(cell(row, 7), Some("Y")),
        is_auto_increment: is_identity,
        is_generated: matches!(cell(row, 9), Some("A")) && !is_identity,
        // SYSCAT.COLUMNS.DEFAULT already holds SQL (`'ACTIVE'`, `1`,
        // `CURRENT TIMESTAMP`), so it doubles as the restatable default.
        default_sql: default_value.clone(),
        default_value,
        character_maximum_length: column_type.character_maximum_length,
        numeric_precision: column_type.numeric_precision,
        numeric_scale: column_type.numeric_scale,
        comment: non_empty(cell(row, 11)),
        full_type: column_type.full_type,
    }
}

/// Column metadata for a table or a view.
pub fn get_columns(
    session: &Session,
    schema: &str,
    table_name: &str,
) -> PluginResult<Vec<TableColumn>> {
    let sql = format!(
        "{COLUMN_SELECT} WHERE TABSCHEMA = {} AND TABNAME = {} AND HIDDEN = '' \
         ORDER BY COLNO WITH UR",
        string_literal(schema),
        string_literal(table_name)
    );
    let result = session.query(&sql)?;
    Ok(result.rows.iter().map(|row| column_from_row(row)).collect())
}

pub fn get_foreign_keys(
    session: &Session,
    schema: &str,
    table_name: &str,
) -> PluginResult<Vec<ForeignKey>> {
    let sql = format!(
        "SELECT R.CONSTNAME, FK.COLNAME, R.REFTABNAME, PK.COLNAME, R.DELETERULE, R.UPDATERULE \
         FROM SYSCAT.REFERENCES R \
         JOIN SYSCAT.KEYCOLUSE FK ON FK.CONSTNAME = R.CONSTNAME AND FK.TABSCHEMA = R.TABSCHEMA AND FK.TABNAME = R.TABNAME \
         JOIN SYSCAT.KEYCOLUSE PK ON PK.CONSTNAME = R.REFKEYNAME AND PK.TABSCHEMA = R.REFTABSCHEMA AND PK.TABNAME = R.REFTABNAME AND PK.COLSEQ = FK.COLSEQ \
         WHERE R.TABSCHEMA = {} AND R.TABNAME = {} ORDER BY R.CONSTNAME, FK.COLSEQ WITH UR",
        string_literal(schema),
        string_literal(table_name)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .map(|row| ForeignKey {
            name: cell(&row, 0).unwrap_or_default().to_string(),
            column_name: cell(&row, 1).unwrap_or_default().to_string(),
            ref_table: cell(&row, 2).unwrap_or_default().to_string(),
            ref_column: cell(&row, 3).unwrap_or_default().to_string(),
            on_delete: cell(&row, 4).map(ToString::to_string),
            on_update: cell(&row, 5).map(ToString::to_string),
        })
        .collect())
}

pub fn get_indexes(session: &Session, schema: &str, table_name: &str) -> PluginResult<Vec<Index>> {
    let sql = format!(
        "SELECT I.INDNAME, C.COLNAME, I.UNIQUERULE, I.INDEXTYPE, C.COLSEQ \
         FROM SYSCAT.INDEXES I \
         JOIN SYSCAT.INDEXCOLUSE C ON C.INDSCHEMA = I.INDSCHEMA AND C.INDNAME = I.INDNAME \
         WHERE I.TABSCHEMA = {} AND I.TABNAME = {} ORDER BY I.INDNAME, C.COLSEQ WITH UR",
        string_literal(schema),
        string_literal(table_name)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .map(|row| Index {
            name: cell(&row, 0).unwrap_or_default().to_string(),
            column_name: cell(&row, 1).unwrap_or_default().to_string(),
            is_unique: matches!(cell(&row, 2), Some("P" | "U")),
            is_primary: matches!(cell(&row, 2), Some("P")),
            seq_in_index: cell(&row, 4)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0),
        })
        .collect())
}

pub fn get_routines(session: &Session, schema: &str) -> PluginResult<Vec<RoutineInfo>> {
    let sql = format!(
        "SELECT ROUTINENAME, ROUTINETYPE, TEXT FROM SYSCAT.ROUTINES WHERE ROUTINESCHEMA = {} ORDER BY ROUTINENAME WITH UR",
        string_literal(schema)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .map(|row| RoutineInfo {
            name: cell(&row, 0).unwrap_or_default().to_string(),
            routine_type: cell(&row, 1).unwrap_or("FUNCTION").to_string(),
            definition: cell(&row, 2).map(ToString::to_string),
        })
        .collect())
}

pub fn get_routine_parameters(
    session: &Session,
    schema: &str,
    routine_name: &str,
) -> PluginResult<Vec<RoutineParameter>> {
    let sql = format!(
        "SELECT PARMNAME, TYPENAME, ROWTYPE, ORDINAL FROM SYSCAT.ROUTINEPARMS \
         WHERE ROUTINESCHEMA = {} AND ROUTINENAME = {} ORDER BY ORDINAL WITH UR",
        string_literal(schema),
        string_literal(routine_name)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .map(|row| RoutineParameter {
            name: cell(&row, 0).unwrap_or_default().to_string(),
            data_type: cell(&row, 1).unwrap_or_default().to_string(),
            mode: match cell(&row, 2).unwrap_or("P") {
                "P" | "B" => "IN".to_string(),
                "O" => "OUT".to_string(),
                "I" => "INOUT".to_string(),
                other => other.to_string(),
            },
            ordinal_position: cell(&row, 3)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0),
        })
        .collect())
}

pub fn get_routine_definition(
    session: &Session,
    schema: &str,
    routine_name: &str,
) -> PluginResult<String> {
    let sql = format!(
        "SELECT TEXT FROM SYSCAT.ROUTINES WHERE ROUTINESCHEMA = {} AND ROUTINENAME = {} FETCH FIRST 1 ROW ONLY WITH UR",
        string_literal(schema),
        string_literal(routine_name)
    );
    let result = session.query(&sql)?;
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next().flatten())
        .unwrap_or_default())
}

/// Column metadata for every table and view of a schema, keyed by table
/// name. `tables` narrows the result to the named tables when given.
pub fn get_all_columns_batch(
    session: &Session,
    schema: &str,
    tables: Option<&[String]>,
) -> PluginResult<HashMap<String, Vec<TableColumn>>> {
    let sql = format!(
        "{COLUMN_SELECT} WHERE TABSCHEMA = {} AND HIDDEN = '' ORDER BY TABNAME, COLNO WITH UR",
        string_literal(schema)
    );
    let result = session.query(&sql)?;
    let mut map: HashMap<String, Vec<TableColumn>> = HashMap::new();
    for row in &result.rows {
        let table_name = cell(row, 0).unwrap_or_default();
        if tables.is_some_and(|wanted| !wanted.iter().any(|name| name == table_name)) {
            continue;
        }
        map.entry(table_name.to_string())
            .or_default()
            .push(column_from_row(row));
    }
    Ok(map)
}

pub fn get_all_foreign_keys_batch(
    session: &Session,
    schema: &str,
    tables: Option<&[String]>,
) -> PluginResult<HashMap<String, Vec<ForeignKey>>> {
    let sql = format!(
        "SELECT R.TABNAME, R.CONSTNAME, FK.COLNAME, R.REFTABNAME, PK.COLNAME, R.DELETERULE, R.UPDATERULE \
         FROM SYSCAT.REFERENCES R \
         JOIN SYSCAT.KEYCOLUSE FK ON FK.CONSTNAME = R.CONSTNAME AND FK.TABSCHEMA = R.TABSCHEMA AND FK.TABNAME = R.TABNAME \
         JOIN SYSCAT.KEYCOLUSE PK ON PK.CONSTNAME = R.REFKEYNAME AND PK.TABSCHEMA = R.REFTABSCHEMA AND PK.TABNAME = R.REFTABNAME AND PK.COLSEQ = FK.COLSEQ \
         WHERE R.TABSCHEMA = {} ORDER BY R.TABNAME, R.CONSTNAME, FK.COLSEQ WITH UR",
        string_literal(schema)
    );
    let result = session.query(&sql)?;
    let mut map: HashMap<String, Vec<ForeignKey>> = HashMap::new();
    for row in &result.rows {
        let table_name = cell(row, 0).unwrap_or_default();
        if tables.is_some_and(|wanted| !wanted.iter().any(|name| name == table_name)) {
            continue;
        }
        map.entry(table_name.to_string())
            .or_default()
            .push(ForeignKey {
                name: cell(row, 1).unwrap_or_default().to_string(),
                column_name: cell(row, 2).unwrap_or_default().to_string(),
                ref_table: cell(row, 3).unwrap_or_default().to_string(),
                ref_column: cell(row, 4).unwrap_or_default().to_string(),
                on_delete: cell(row, 5).map(ToString::to_string),
                on_update: cell(row, 6).map(ToString::to_string),
            });
    }
    Ok(map)
}

/// Every table of a schema with its columns and foreign keys, for the ER
/// diagram. Three catalog queries on one connection, whatever the number of
/// tables.
pub fn get_schema_snapshot(session: &Session, schema: &str) -> PluginResult<Vec<TableSchema>> {
    let tables = get_tables(session, schema)?;
    let mut columns_map = get_all_columns_batch(session, schema, None)?;
    let mut fks_map = get_all_foreign_keys_batch(session, schema, None)?;
    Ok(tables
        .into_iter()
        .map(|table| TableSchema {
            columns: columns_map.remove(&table.name).unwrap_or_default(),
            foreign_keys: fks_map.remove(&table.name).unwrap_or_default(),
            name: table.name,
        })
        .collect())
}

/// Reconstructs the `CREATE TABLE` of an existing table, with its indexes
/// and comments, for "Export table to SQL" and "Dump Database".
///
/// Columns come with their full type, nullability, default, identity and
/// generation clause; the primary key and unique constraints are part of
/// the statement. Foreign keys are left out on purpose: a dump creates
/// tables one after the other, and a reference to a table that comes later
/// would stop it.
pub fn get_table_ddl(session: &Session, schema: &str, table_name: &str) -> PluginResult<String> {
    let schema_literal = string_literal(schema);
    let table_literal = string_literal(table_name);
    let target = quote_qualified_name(Some(schema), table_name);

    let kind = session.query(&format!(
        "SELECT TYPE, REMARKS FROM SYSCAT.TABLES \
         WHERE TABSCHEMA = {schema_literal} AND TABNAME = {table_literal} WITH UR"
    ))?;
    let Some(table_row) = kind.rows.first() else {
        return Err(format!("Table {target} was not found"));
    };
    match cell(table_row, 0) {
        Some("T" | "U" | "S" | "G") => {}
        Some("V") => return Err(format!("{target} is a view, not a table")),
        _ => {
            return Err(format!(
                "{target} is not a table whose definition can be rebuilt"
            ))
        }
    }
    let table_comment = non_empty(cell(table_row, 1));

    let columns = session.query(&format!(
        "SELECT COLNAME, TYPENAME, LENGTH, SCALE, CODEPAGE, NULLS, IDENTITY, GENERATED, DEFAULT, \
                REMARKS, HIDDEN, ROWCHANGETIMESTAMP, TEXT, TYPESCHEMA \
         FROM SYSCAT.COLUMNS WHERE TABSCHEMA = {schema_literal} AND TABNAME = {table_literal} \
         ORDER BY COLNO WITH UR"
    ))?;

    let mut definitions = Vec::new();
    let mut comments = Vec::new();
    for row in &columns.rows {
        let name = cell(row, 0).unwrap_or_default();
        let described = describe_catalog_type(
            cell(row, 1).unwrap_or_default(),
            cell_i64(row, 2),
            cell_i64(row, 3),
            cell_i64(row, 4),
        );
        let mut type_sql = described.full_type.unwrap_or(described.data_type);
        // A user-defined type lives in a schema of its own.
        if let Some(type_schema) = cell(row, 13).map(str::trim) {
            if !type_schema.starts_with("SYS") {
                type_sql = quote_qualified_name(Some(type_schema), &type_sql);
            }
        }

        let mut definition = format!("{} {type_sql}", quote_identifier(name));
        if !matches!(cell(row, 5), Some("Y")) {
            definition.push_str(" NOT NULL");
        }
        let generated = match cell(row, 7) {
            Some("A") => Some("ALWAYS"),
            Some("D") => Some("BY DEFAULT"),
            _ => None,
        };
        if matches!(cell(row, 6), Some("Y")) {
            definition.push_str(&format!(
                " GENERATED {} AS IDENTITY",
                generated.unwrap_or("ALWAYS")
            ));
        } else if matches!(cell(row, 11), Some("Y")) {
            definition.push_str(&format!(
                " GENERATED {} FOR EACH ROW ON UPDATE AS ROW CHANGE TIMESTAMP",
                generated.unwrap_or("ALWAYS")
            ));
        } else if let (Some(mode), Some(expression)) = (generated, non_empty(cell(row, 12))) {
            // SYSCAT.COLUMNS.TEXT holds the clause from AS on: `AS (expr)`.
            definition.push_str(&format!(" GENERATED {mode} {}", expression.trim()));
        } else if let Some(default) = non_empty(cell(row, 8)) {
            definition.push_str(&format!(" DEFAULT {}", default.trim()));
        }
        if matches!(cell(row, 10), Some("I")) {
            definition.push_str(" IMPLICITLY HIDDEN");
        }
        definitions.push(definition);

        if let Some(comment) = non_empty(cell(row, 9)) {
            comments.push(format!(
                "COMMENT ON COLUMN {target}.{} IS {}",
                quote_identifier(name),
                string_literal(&comment)
            ));
        }
    }
    if definitions.is_empty() {
        return Err(format!("Table {target} has no columns"));
    }

    // Primary key and unique constraints, in key order.
    let keys = session.query(&format!(
        "SELECT TC.CONSTNAME, TC.TYPE, K.COLNAME \
         FROM SYSCAT.TABCONST TC \
         JOIN SYSCAT.KEYCOLUSE K ON K.CONSTNAME = TC.CONSTNAME AND K.TABSCHEMA = TC.TABSCHEMA \
              AND K.TABNAME = TC.TABNAME \
         WHERE TC.TABSCHEMA = {schema_literal} AND TC.TABNAME = {table_literal} \
               AND TC.TYPE IN ('P', 'U') \
         ORDER BY TC.TYPE, TC.CONSTNAME, K.COLSEQ WITH UR"
    ))?;
    let mut constraints: Vec<(String, String, Vec<String>)> = Vec::new();
    for row in &keys.rows {
        let name = cell(row, 0).unwrap_or_default().to_string();
        let kind = cell(row, 1).unwrap_or_default().to_string();
        let column = quote_identifier(cell(row, 2).unwrap_or_default());
        match constraints.last_mut() {
            Some((last, _, columns)) if *last == name => columns.push(column),
            _ => constraints.push((name, kind, vec![column])),
        }
    }
    for (name, kind, key_columns) in &constraints {
        let clause = if kind == "P" { "PRIMARY KEY" } else { "UNIQUE" };
        // Db2 names an unnamed constraint SQL<timestamp>; restating that
        // name would only make the statement harder to reuse.
        let prefix = if name.starts_with("SQL") && name[3..].bytes().all(|b| b.is_ascii_digit()) {
            String::new()
        } else {
            format!("CONSTRAINT {} ", quote_identifier(name))
        };
        definitions.push(format!("{prefix}{clause} ({})", key_columns.join(", ")));
    }

    let mut statements = vec![format!(
        "CREATE TABLE {target} (\n  {}\n)",
        definitions.join(",\n  ")
    )];

    // Indexes the user created; the ones backing a key are implied by it.
    let indexes = session.query(&format!(
        "SELECT I.INDSCHEMA, I.INDNAME, I.UNIQUERULE, C.COLNAME, C.COLORDER \
         FROM SYSCAT.INDEXES I \
         JOIN SYSCAT.INDEXCOLUSE C ON C.INDSCHEMA = I.INDSCHEMA AND C.INDNAME = I.INDNAME \
         WHERE I.TABSCHEMA = {schema_literal} AND I.TABNAME = {table_literal} \
               AND I.UNIQUERULE <> 'P' AND I.SYSTEM_REQUIRED = 0 \
         ORDER BY I.INDNAME, C.COLSEQ WITH UR"
    ))?;
    let mut index_list: Vec<(String, bool, Vec<String>)> = Vec::new();
    for row in &indexes.rows {
        let name = quote_qualified_name(
            cell(row, 0).map(str::trim),
            cell(row, 1).unwrap_or_default(),
        );
        let unique = matches!(cell(row, 2), Some("U"));
        let mut column = quote_identifier(cell(row, 3).unwrap_or_default());
        if matches!(cell(row, 4), Some("D")) {
            column.push_str(" DESC");
        }
        match index_list.last_mut() {
            Some((last, _, columns)) if *last == name => columns.push(column),
            _ => index_list.push((name, unique, vec![column])),
        }
    }
    for (name, unique, index_columns) in index_list {
        statements.push(format!(
            "CREATE {}INDEX {name} ON {target} ({})",
            if unique { "UNIQUE " } else { "" },
            index_columns.join(", ")
        ));
    }

    if let Some(comment) = table_comment {
        statements.push(format!(
            "COMMENT ON TABLE {target} IS {}",
            string_literal(&comment)
        ));
    }
    statements.extend(comments);

    Ok(statements
        .iter()
        .map(|statement| format!("{statement};"))
        .collect::<Vec<_>>()
        .join("\n\n"))
}
