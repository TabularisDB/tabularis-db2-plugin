use crate::models::ColumnDefinition;
use crate::utils::identifiers::{quote_identifier, quote_qualified_name};
use crate::utils::types::column_type_sql;

pub fn get_create_table_sql(
    table_name: &str,
    columns: &[ColumnDefinition],
    schema: Option<&str>,
) -> Vec<String> {
    let mut definitions = columns.iter().map(column_type_sql).collect::<Vec<_>>();

    let primary_keys = columns
        .iter()
        .filter(|column| column.is_pk)
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>();

    if !primary_keys.is_empty() {
        definitions.push(format!("PRIMARY KEY ({})", primary_keys.join(", ")));
    }

    vec![format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_qualified_name(schema, table_name),
        definitions.join(",\n  ")
    )]
}

pub fn get_add_column_sql(
    table: &str,
    column: &ColumnDefinition,
    schema: Option<&str>,
) -> Vec<String> {
    vec![format!(
        "ALTER TABLE {} ADD COLUMN {}",
        quote_qualified_name(schema, table),
        column_type_sql(column)
    )]
}

pub fn get_alter_column_sql(
    table: &str,
    old_column: &ColumnDefinition,
    new_column: &ColumnDefinition,
    schema: Option<&str>,
) -> Vec<String> {
    let qualified = quote_qualified_name(schema, table);
    let mut statements = Vec::new();

    if old_column.name != new_column.name {
        statements.push(format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            qualified,
            quote_identifier(&old_column.name),
            quote_identifier(&new_column.name)
        ));
    }

    if old_column.data_type != new_column.data_type
        || old_column.is_nullable != new_column.is_nullable
        || old_column.default_value != new_column.default_value
    {
        statements.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} SET DATA TYPE {}",
            qualified,
            quote_identifier(&new_column.name),
            new_column.data_type
        ));
    }

    if statements.is_empty() {
        statements.push("-- No changes needed".to_string());
    }

    statements
}

pub fn get_create_index_sql(
    table: &str,
    index_name: &str,
    columns: &[String],
    is_unique: bool,
    schema: Option<&str>,
) -> Vec<String> {
    let unique = if is_unique { "UNIQUE " } else { "" };
    let columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "CREATE {}INDEX {} ON {} ({})",
        unique,
        quote_identifier(index_name),
        quote_qualified_name(schema, table),
        columns
    )]
}

#[allow(clippy::too_many_arguments)]
pub fn get_create_foreign_key_sql(
    table: &str,
    fk_name: &str,
    column: &str,
    ref_table: &str,
    ref_column: &str,
    on_delete: Option<&str>,
    on_update: Option<&str>,
    schema: Option<&str>,
) -> Vec<String> {
    let mut sql = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        quote_qualified_name(schema, table),
        quote_identifier(fk_name),
        quote_identifier(column),
        quote_qualified_name(schema, ref_table),
        quote_identifier(ref_column)
    );
    if let Some(rule) = on_delete {
        sql.push_str(&format!(" ON DELETE {rule}"));
    }
    if let Some(rule) = on_update {
        sql.push_str(&format!(" ON UPDATE {rule}"));
    }
    vec![sql]
}

pub fn drop_index_sql(_table: &str, index_name: &str, schema: Option<&str>) -> String {
    format!(
        "DROP INDEX {}.{}",
        schema.unwrap_or(""),
        quote_identifier(index_name)
    )
    .trim_start_matches('.')
    .to_string()
}

pub fn drop_foreign_key_sql(table: &str, fk_name: &str, schema: Option<&str>) -> String {
    format!(
        "ALTER TABLE {} DROP FOREIGN KEY {}",
        quote_qualified_name(schema, table),
        quote_identifier(fk_name)
    )
}

#[cfg(test)]
mod tests {
    use super::{get_add_column_sql, get_create_foreign_key_sql, get_create_table_sql};
    use crate::models::ColumnDefinition;

    fn make_column(name: &str, data_type: &str, is_pk: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            data_type: data_type.to_string(),
            is_nullable: !is_pk,
            is_pk,
            is_auto_increment: false,
            default_value: None,
        }
    }

    #[test]
    fn creates_table_sql() {
        let sql = get_create_table_sql(
            "USERS",
            &[
                make_column("ID", "INTEGER", true),
                make_column("NAME", "VARCHAR(255)", false),
            ],
            Some("APP"),
        );
        assert!(sql[0].contains("CREATE TABLE \"APP\".\"USERS\""));
        assert!(sql[0].contains("PRIMARY KEY"));
    }

    #[test]
    fn creates_add_column_sql() {
        let sql = get_add_column_sql("USERS", &make_column("AGE", "INTEGER", false), Some("APP"));
        assert!(sql[0].contains("ADD COLUMN"));
    }

    #[test]
    fn creates_foreign_key_sql() {
        let sql = get_create_foreign_key_sql(
            "ORDERS",
            "FK_ORDERS_USER",
            "USER_ID",
            "USERS",
            "ID",
            Some("CASCADE"),
            None,
            Some("APP"),
        );
        assert!(sql[0].contains("FOREIGN KEY"));
        assert!(sql[0].contains("ON DELETE CASCADE"));
    }
}
