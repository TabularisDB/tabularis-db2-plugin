use crate::client::query_text_rows;
use crate::error::PluginResult;
use crate::models::{
    ConnectionParams, ForeignKey, Index, PluginSettings, RoutineInfo, RoutineParameter,
    TableColumn, TableInfo, TableSchema, ViewInfo,
};
use crate::utils::values::bool_from_catalog_flag;
use std::collections::HashMap;

fn cell(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index).and_then(|value| value.as_deref())
}

pub fn get_databases(
    params: &ConnectionParams,
    settings: &PluginSettings,
) -> PluginResult<Vec<String>> {
    let result = query_text_rows(
        params,
        settings,
        None,
        "SELECT CURRENT SERVER AS NAME FROM SYSIBM.SYSDUMMY1 WITH UR",
    )?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

pub fn get_schemas(
    params: &ConnectionParams,
    settings: &PluginSettings,
) -> PluginResult<Vec<String>> {
    let result = query_text_rows(
        params,
        settings,
        None,
        "SELECT DISTINCT RTRIM(SCHEMANAME) FROM (\
            SELECT SCHEMANAME FROM SYSCAT.SCHEMATA WHERE SCHEMANAME NOT LIKE 'SYS%' \
            UNION \
            SELECT TABSCHEMA FROM SYSCAT.TABLES WHERE TABSCHEMA NOT LIKE 'SYS%' AND TYPE IN ('T', 'V') \
        ) AS S ORDER BY 1 WITH UR",
    )?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect())
}

pub fn get_tables(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<Vec<TableInfo>> {
    let sql = format!(
        "SELECT TABNAME, REMARKS FROM SYSCAT.TABLES WHERE TABSCHEMA = '{}' AND TYPE = 'T' ORDER BY TABNAME WITH UR",
        schema.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| {
            cell(&row, 0).map(|name| TableInfo {
                name: name.to_string(),
                comment: cell(&row, 1).map(ToString::to_string),
            })
        })
        .collect())
}

pub fn get_views(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<Vec<ViewInfo>> {
    let sql = format!(
        "SELECT VIEWNAME, TEXT FROM SYSCAT.VIEWS WHERE VIEWSCHEMA = '{}' ORDER BY VIEWNAME WITH UR",
        schema.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    Ok(result
        .rows
        .into_iter()
        .filter_map(|row| {
            cell(&row, 0).map(|name| ViewInfo {
                name: name.to_string(),
                definition: cell(&row, 1).map(ToString::to_string),
            })
        })
        .collect())
}

pub fn get_view_definition(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    view_name: &str,
) -> PluginResult<String> {
    let sql = format!(
        "SELECT TEXT FROM SYSCAT.VIEWS WHERE VIEWSCHEMA = '{}' AND VIEWNAME = '{}' WITH UR",
        schema.replace('\'', "''"),
        view_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next().flatten())
        .unwrap_or_default())
}

pub fn get_columns(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    table_name: &str,
) -> PluginResult<Vec<TableColumn>> {
    let sql = format!(
        "SELECT COLNAME, TYPENAME, KEYSEQ, NULLS, IDENTITY, DEFAULT, LENGTH, REMARKS \
         FROM SYSCAT.COLUMNS WHERE TABSCHEMA = '{}' AND TABNAME = '{}' ORDER BY COLNO WITH UR",
        schema.replace('\'', "''"),
        table_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    Ok(result
        .rows
        .into_iter()
        .map(|row| TableColumn {
            name: cell(&row, 0).unwrap_or_default().to_string(),
            data_type: cell(&row, 1).unwrap_or_default().to_string(),
            is_pk: cell(&row, 2)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0)
                > 0,
            is_nullable: matches!(cell(&row, 3), Some("Y")),
            is_auto_increment: matches!(cell(&row, 4), Some("Y")),
            default_value: cell(&row, 5).map(ToString::to_string),
            character_maximum_length: cell(&row, 6).and_then(|value| value.parse::<u64>().ok()),
            comment: cell(&row, 7).map(ToString::to_string),
        })
        .collect())
}

pub fn get_foreign_keys(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    table_name: &str,
) -> PluginResult<Vec<ForeignKey>> {
    let sql = format!(
        "SELECT R.CONSTNAME, FK.COLNAME, R.REFTABNAME, PK.COLNAME, R.DELETERULE, R.UPDATERULE \
         FROM SYSCAT.REFERENCES R \
         JOIN SYSCAT.KEYCOLUSE FK ON FK.CONSTNAME = R.CONSTNAME AND FK.TABSCHEMA = R.TABSCHEMA AND FK.TABNAME = R.TABNAME \
         JOIN SYSCAT.KEYCOLUSE PK ON PK.CONSTNAME = R.REFKEYNAME AND PK.TABSCHEMA = R.REFTABSCHEMA AND PK.TABNAME = R.REFTABNAME AND PK.COLSEQ = FK.COLSEQ \
         WHERE R.TABSCHEMA = '{}' AND R.TABNAME = '{}' ORDER BY R.CONSTNAME, FK.COLSEQ WITH UR",
        schema.replace('\'', "''"),
        table_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
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

pub fn get_indexes(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    table_name: &str,
) -> PluginResult<Vec<Index>> {
    let sql = format!(
        "SELECT I.INDNAME, C.COLNAME, I.UNIQUERULE, I.INDEXTYPE, C.COLSEQ \
         FROM SYSCAT.INDEXES I \
         JOIN SYSCAT.INDEXCOLUSE C ON C.INDSCHEMA = I.INDSCHEMA AND C.INDNAME = I.INDNAME \
         WHERE I.TABSCHEMA = '{}' AND I.TABNAME = '{}' ORDER BY I.INDNAME, C.COLSEQ WITH UR",
        schema.replace('\'', "''"),
        table_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
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

pub fn get_routines(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<Vec<RoutineInfo>> {
    let sql = format!(
        "SELECT ROUTINENAME, ROUTINETYPE, TEXT FROM SYSCAT.ROUTINES WHERE ROUTINESCHEMA = '{}' ORDER BY ROUTINENAME WITH UR",
        schema.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
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
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    routine_name: &str,
) -> PluginResult<Vec<RoutineParameter>> {
    let sql = format!(
        "SELECT PARMNAME, TYPENAME, ROWTYPE, ORDINAL FROM SYSCAT.ROUTINEPARMS \
         WHERE ROUTINESCHEMA = '{}' AND ROUTINENAME = '{}' ORDER BY ORDINAL WITH UR",
        schema.replace('\'', "''"),
        routine_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
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
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
    routine_name: &str,
) -> PluginResult<String> {
    let sql = format!(
        "SELECT TEXT FROM SYSCAT.ROUTINES WHERE ROUTINESCHEMA = '{}' AND ROUTINENAME = '{}' FETCH FIRST 1 ROW ONLY WITH UR",
        schema.replace('\'', "''"),
        routine_name.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next().flatten())
        .unwrap_or_default())
}

pub fn get_all_columns_batch(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<HashMap<String, Vec<TableColumn>>> {
    let sql = format!(
        "SELECT TABNAME, COLNAME, TYPENAME, KEYSEQ, NULLS, IDENTITY, DEFAULT, LENGTH, REMARKS \
         FROM SYSCAT.COLUMNS WHERE TABSCHEMA = '{}' ORDER BY TABNAME, COLNO WITH UR",
        schema.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    let mut map = HashMap::new();
    for row in result.rows {
        let table_name = cell(&row, 0).unwrap_or_default().to_string();
        map.entry(table_name)
            .or_insert_with(Vec::new)
            .push(TableColumn {
                name: cell(&row, 1).unwrap_or_default().to_string(),
                data_type: cell(&row, 2).unwrap_or_default().to_string(),
                is_pk: cell(&row, 3)
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0)
                    > 0,
                is_nullable: bool_from_catalog_flag(cell(&row, 4)),
                is_auto_increment: bool_from_catalog_flag(cell(&row, 5)),
                default_value: cell(&row, 6).map(ToString::to_string),
                character_maximum_length: cell(&row, 7).and_then(|value| value.parse::<u64>().ok()),
                comment: cell(&row, 8).map(ToString::to_string),
            });
    }
    Ok(map)
}

pub fn get_all_foreign_keys_batch(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<HashMap<String, Vec<ForeignKey>>> {
    let sql = format!(
        "SELECT R.TABNAME, R.CONSTNAME, FK.COLNAME, R.REFTABNAME, PK.COLNAME, R.DELETERULE, R.UPDATERULE \
         FROM SYSCAT.REFERENCES R \
         JOIN SYSCAT.KEYCOLUSE FK ON FK.CONSTNAME = R.CONSTNAME AND FK.TABSCHEMA = R.TABSCHEMA AND FK.TABNAME = R.TABNAME \
         JOIN SYSCAT.KEYCOLUSE PK ON PK.CONSTNAME = R.REFKEYNAME AND PK.TABSCHEMA = R.REFTABSCHEMA AND PK.TABNAME = R.REFTABNAME AND PK.COLSEQ = FK.COLSEQ \
         WHERE R.TABSCHEMA = '{}' ORDER BY R.TABNAME, R.CONSTNAME, FK.COLSEQ WITH UR",
        schema.replace('\'', "''")
    );
    let result = query_text_rows(params, settings, Some(schema), &sql)?;
    let mut map = HashMap::new();
    for row in result.rows {
        let table_name = cell(&row, 0).unwrap_or_default().to_string();
        map.entry(table_name)
            .or_insert_with(Vec::new)
            .push(ForeignKey {
                name: cell(&row, 1).unwrap_or_default().to_string(),
                column_name: cell(&row, 2).unwrap_or_default().to_string(),
                ref_table: cell(&row, 3).unwrap_or_default().to_string(),
                ref_column: cell(&row, 4).unwrap_or_default().to_string(),
                on_delete: cell(&row, 5).map(ToString::to_string),
                on_update: cell(&row, 6).map(ToString::to_string),
            });
    }
    Ok(map)
}

pub fn get_schema_snapshot(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema: &str,
) -> PluginResult<Vec<TableSchema>> {
    let tables = get_tables(params, settings, schema)?;
    let columns_map = get_all_columns_batch(params, settings, schema)?;
    let fks_map = get_all_foreign_keys_batch(params, settings, schema)?;
    Ok(tables
        .into_iter()
        .map(|table| TableSchema {
            name: table.name.clone(),
            columns: columns_map.get(&table.name).cloned().unwrap_or_default(),
            foreign_keys: fks_map.get(&table.name).cloned().unwrap_or_default(),
        })
        .collect())
}
