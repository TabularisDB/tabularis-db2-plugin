use crate::error::{err_to_string, PluginResult};
use crate::models::{ConnectionParams, PluginSettings};
use odbc_api::handles::Statement;
use odbc_api::{ConnectionOptions, Cursor, Environment};
use std::ffi::c_void;
use std::sync::OnceLock;

type OdbcHandle = *mut c_void;
type OdbcLen = isize;

#[cfg_attr(windows, link(name = "odbc32"))]
#[cfg_attr(not(windows), link(name = "odbc"))]
extern "C" {
    fn SQLFetch(stmt: OdbcHandle) -> i16;
    fn SQLGetData(
        stmt: OdbcHandle,
        col: u16,
        target_type: i16,
        target_ptr: *mut c_void,
        buffer_length: OdbcLen,
        indicator: *mut OdbcLen,
    ) -> i16;
}

const SQL_C_CHAR: i16 = 1;
const SQL_SUCCESS: i16 = 0;
const SQL_SUCCESS_WITH_INFO: i16 = 1;
const SQL_NO_DATA: i16 = 100;
const SQL_NULL_DATA: OdbcLen = -1;

static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

#[derive(Debug)]
pub struct TextQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

fn environment() -> PluginResult<&'static Environment> {
    if let Some(environment) = ENVIRONMENT.get() {
        return Ok(environment);
    }

    let environment = Environment::new().map_err(err_to_string)?;
    let _ = ENVIRONMENT.set(environment);
    ENVIRONMENT
        .get()
        .ok_or_else(|| "Failed to initialize the ODBC environment".to_string())
}

pub fn build_connection_string(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema_override: Option<&str>,
) -> String {
    let mut parts = vec![format!(
        "Driver={{{}}}",
        settings
            .driver_name
            .clone()
            .unwrap_or_else(|| "IBM DB2 ODBC DRIVER".to_string())
    )];

    if let Some(host) = &params.host {
        parts.push(format!("Hostname={host}"));
    }
    parts.push(format!("Port={}", params.port.unwrap_or(50000)));
    parts.push("Protocol=TCPIP".to_string());
    parts.push(format!("Database={}", params.database.primary()));

    if let Some(username) = &params.username {
        parts.push(format!("Uid={username}"));
    }
    if let Some(password) = &params.password {
        parts.push(format!("Pwd={password}"));
    }

    let security = settings
        .security
        .as_deref()
        .or(params.ssl_mode.as_deref())
        .unwrap_or("none");
    if matches!(security, "ssl" | "SSL" | "require") {
        parts.push("Security=SSL".to_string());
    }

    if let Some(schema) = schema_override
        .filter(|value| !value.is_empty())
        .or(settings
            .current_schema
            .as_deref()
            .filter(|value| !value.is_empty()))
    {
        parts.push(format!("CurrentSchema={schema}"));
    }

    if let Some(extra) = settings
        .extra_properties
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        parts.push(extra.to_string());
    }

    parts.join(";")
}

fn fetch_text_rows(mut cursor: impl Cursor) -> PluginResult<TextQueryResult> {
    let num_cols = cursor.num_result_cols().map_err(err_to_string)? as u16;
    let columns = cursor
        .column_names()
        .map_err(err_to_string)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err_to_string)?;

    // Use raw ODBC calls to work around the IBM DB2 clidriver writing only
    // 32-bit indicator values on a 64-bit platform. odbc-api initializes
    // indicators to non-zero values (e.g. SQL_NO_TOTAL = -4), and the driver
    // only overwrites the lower 32 bits, leaving garbage in the upper bits.
    // By zero-initializing the indicator before each SQLGetData call, the
    // upper bits stay zero and the 32-bit value is correctly zero-extended.
    let hstmt = cursor.as_stmt_ref().as_sys();
    let hstmt_ptr = hstmt.0 as OdbcHandle;
    let mut rows = Vec::new();
    let mut buf = vec![0u8; 4096];

    loop {
        let ret = unsafe { SQLFetch(hstmt_ptr) };
        if ret == SQL_NO_DATA {
            break;
        }
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(format!("SQLFetch failed with return code {ret}"));
        }

        let mut values = Vec::with_capacity(num_cols as usize);
        for col in 1..=num_cols {
            buf.fill(0);
            let mut indicator: OdbcLen = 0;
            let ret = unsafe {
                SQLGetData(
                    hstmt_ptr,
                    col,
                    SQL_C_CHAR,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len() as OdbcLen,
                    &mut indicator,
                )
            };
            if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
                return Err(format!(
                    "SQLGetData failed for column {col} with code {ret}"
                ));
            }
            // Mask to 32 bits to handle DB2 clidriver writing only lower 32 bits
            let indicator = (indicator as i32) as OdbcLen;
            if indicator == SQL_NULL_DATA {
                values.push(None);
            } else if indicator >= 0 {
                let len = (indicator as usize).min(buf.len() - 1);
                values.push(Some(String::from_utf8_lossy(&buf[..len]).into_owned()));
            } else {
                values.push(None);
            }
        }
        rows.push(values);
    }

    Ok(TextQueryResult { columns, rows })
}

pub fn query_text_rows(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema_override: Option<&str>,
    sql: &str,
) -> PluginResult<TextQueryResult> {
    let env = environment()?;
    let connection_string = build_connection_string(params, settings, schema_override);
    let connection = env
        .connect_with_connection_string(&connection_string, ConnectionOptions::default())
        .map_err(err_to_string)?;
    let maybe_cursor = connection.execute(sql, (), None).map_err(err_to_string)?;
    let cursor = maybe_cursor.ok_or_else(|| "The query did not return a result set".to_string())?;
    fetch_text_rows(cursor)
}

pub fn execute_statement(
    params: &ConnectionParams,
    settings: &PluginSettings,
    schema_override: Option<&str>,
    sql: &str,
) -> PluginResult<u64> {
    let env = environment()?;
    let connection_string = build_connection_string(params, settings, schema_override);
    let connection = env
        .connect_with_connection_string(&connection_string, ConnectionOptions::default())
        .map_err(err_to_string)?;
    let _ = connection.execute(sql, (), None).map_err(err_to_string)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::build_connection_string;
    use crate::models::{ConnectionParams, DatabaseSelection, PluginSettings};

    #[test]
    fn builds_db2_connection_string() {
        let params = ConnectionParams {
            driver: "db2".to_string(),
            host: Some("localhost".to_string()),
            port: Some(50000),
            username: Some("db2inst1".to_string()),
            password: Some("secret".to_string()),
            database: DatabaseSelection::Single("sample".to_string()),
            ssl_mode: None,
        };
        let settings = PluginSettings::default();
        let connection_string = build_connection_string(&params, &settings, Some("APP"));
        assert!(connection_string.contains("Database=sample"));
        assert!(connection_string.contains("CurrentSchema=APP"));
    }
}
