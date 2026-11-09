use crate::error::{odbc_error, PluginResult};
use crate::models::{ConnectionParams, PluginSettings};
use crate::utils::values::string_literal;
use odbc_api::handles::Statement;
use odbc_api::{Connection, ConnectionOptions, Cursor, Environment};
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
const SQL_NO_TOTAL: OdbcLen = -4;

/// Size of the buffer one `SQLGetData` call fills. Longer values (CLOBs,
/// view and routine bodies, long comments) are read in several calls.
const FETCH_CHUNK_BYTES: usize = 8192;

static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

/// A result set fetched as text: one `Option<String>` per cell, `None` for
/// SQL NULL.
#[derive(Debug, Default)]
pub struct TextQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

fn environment() -> PluginResult<&'static Environment> {
    if let Some(environment) = ENVIRONMENT.get() {
        return Ok(environment);
    }

    let environment = Environment::new().map_err(odbc_error)?;
    let _ = ENVIRONMENT.set(environment);
    ENVIRONMENT
        .get()
        .ok_or_else(|| "Failed to initialize the ODBC environment".to_string())
}

/// Quotes a connection-string value when it contains characters that would
/// otherwise end it (`;`) or change its meaning (`{`, `}`, leading or
/// trailing spaces). ODBC escapes a closing brace inside braces by doubling
/// it.
fn connection_string_value(value: &str) -> String {
    let needs_braces = value.contains([';', '{', '}'])
        || value.starts_with(' ')
        || value.ends_with(' ')
        || value.is_empty();
    if needs_braces {
        format!("{{{}}}", value.replace('}', "}}"))
    } else {
        value.to_string()
    }
}

pub fn build_connection_string(params: &ConnectionParams, settings: &PluginSettings) -> String {
    let mut parts = vec![format!(
        "Driver={{{}}}",
        settings
            .driver_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("IBM DB2 ODBC DRIVER")
    )];

    if let Some(host) = params.host.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("Hostname={}", connection_string_value(host)));
    }
    parts.push(format!("Port={}", params.port.unwrap_or(50000)));
    parts.push("Protocol=TCPIP".to_string());
    parts.push(format!(
        "Database={}",
        connection_string_value(params.database.primary())
    ));

    if let Some(username) = &params.username {
        parts.push(format!("Uid={}", connection_string_value(username)));
    }
    if let Some(password) = &params.password {
        parts.push(format!("Pwd={}", connection_string_value(password)));
    }

    let security = settings
        .security
        .as_deref()
        .or(params.ssl_mode.as_deref())
        .unwrap_or("none");
    if matches!(security, "ssl" | "SSL" | "require") {
        parts.push("Security=SSL".to_string());
    }

    if let Some(extra) = settings
        .extra_properties
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(extra.trim_matches(';').to_string());
    }

    parts.join(";")
}

/// One ODBC connection to Db2, opened for the duration of a single RPC.
///
/// Every statement of a request runs on the same connection, so the special
/// registers set when it is opened (`CURRENT SCHEMA`) stay in effect and
/// multi-step operations see a consistent session.
pub struct Session {
    connection: Connection<'static>,
}

impl Session {
    /// Connects and, when `schema` is given, makes it the current schema so
    /// unqualified names in the user's SQL resolve there.
    ///
    /// The schema is set with a `SET CURRENT SCHEMA` string constant rather
    /// than the `CurrentSchema` connection keyword: a string constant is not
    /// folded to upper case, so schemas created with a delimited, mixed-case
    /// or otherwise special name work too.
    pub fn open(
        params: &ConnectionParams,
        settings: &PluginSettings,
        schema: Option<&str>,
    ) -> PluginResult<Self> {
        let env = environment()?;
        let connection_string = build_connection_string(params, settings);
        let options = ConnectionOptions {
            login_timeout_sec: params.connect_timeout_secs,
            ..ConnectionOptions::default()
        };
        let connection = env
            .connect_with_connection_string(&connection_string, options)
            .map_err(odbc_error)?;
        let session = Self { connection };

        let schema = schema.filter(|value| !value.is_empty()).or(settings
            .current_schema
            .as_deref()
            .filter(|value| !value.is_empty()));
        if let Some(schema) = schema {
            session.execute(&format!("SET CURRENT SCHEMA = {}", string_literal(schema)))?;
        }

        Ok(session)
    }

    /// Runs a statement that must produce a result set and fetches all of it.
    pub fn query(&self, sql: &str) -> PluginResult<TextQueryResult> {
        let cursor = self
            .connection
            .execute(sql, (), None)
            .map_err(odbc_error)?
            .ok_or_else(|| "The query did not return a result set".to_string())?;
        fetch_text_rows(cursor)
    }

    /// Runs a statement for its side effect and returns the number of rows it
    /// changed (0 when Db2 reports none, e.g. for DDL).
    pub fn execute(&self, sql: &str) -> PluginResult<u64> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        // A statement that happens to return rows (a CALL with a result set)
        // is still a success; the cursor is dropped unread.
        if statement.execute(sql, ()).map_err(odbc_error)?.is_some() {
            return Ok(0);
        }
        let affected = statement.row_count().map_err(odbc_error)?.unwrap_or(0);
        Ok(affected as u64)
    }
}

/// Reads one cell as text, however long it is.
///
/// Raw ODBC calls work around the IBM Db2 clidriver writing only 32-bit
/// length indicators on a 64-bit platform: `odbc-api` initializes indicators
/// to non-zero values, and the driver only overwrites the lower 32 bits,
/// leaving garbage in the upper ones. Zero-initializing the indicator before
/// each call and reading it back as an `i32` yields the real value.
///
/// A value longer than the buffer comes back in pieces: `SQLGetData` returns
/// `SQL_SUCCESS_WITH_INFO` with the buffer full, and the next call continues
/// where the previous one stopped. The bytes are joined before decoding so a
/// multi-byte character split across two pieces survives.
fn read_cell(
    statement: OdbcHandle,
    column: u16,
    buffer: &mut [u8],
) -> PluginResult<Option<String>> {
    let capacity = buffer.len() - 1; // the driver NUL-terminates each piece
    let mut bytes: Vec<u8> = Vec::new();

    loop {
        let mut indicator: OdbcLen = 0;
        let ret = unsafe {
            SQLGetData(
                statement,
                column,
                SQL_C_CHAR,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as OdbcLen,
                &mut indicator,
            )
        };
        if ret == SQL_NO_DATA {
            break;
        }
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(format!(
                "SQLGetData failed for column {column} with code {ret}"
            ));
        }

        let indicator = (indicator as i32) as OdbcLen;
        if indicator == SQL_NULL_DATA {
            return Ok(None);
        }

        let truncated = ret == SQL_SUCCESS_WITH_INFO
            && (indicator == SQL_NO_TOTAL || indicator as usize > capacity);
        let piece = if truncated {
            capacity
        } else {
            (indicator.max(0) as usize).min(capacity)
        };
        bytes.extend_from_slice(&buffer[..piece]);

        if !truncated {
            break;
        }
    }

    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn fetch_text_rows(mut cursor: impl Cursor) -> PluginResult<TextQueryResult> {
    let column_count = cursor.num_result_cols().map_err(odbc_error)? as u16;
    let columns = cursor
        .column_names()
        .map_err(odbc_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(odbc_error)?;

    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
    let mut rows = Vec::new();
    let mut buffer = vec![0u8; FETCH_CHUNK_BYTES];

    loop {
        let ret = unsafe { SQLFetch(statement) };
        if ret == SQL_NO_DATA {
            break;
        }
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(format!("SQLFetch failed with return code {ret}"));
        }

        let mut values = Vec::with_capacity(column_count as usize);
        for column in 1..=column_count {
            values.push(read_cell(statement, column, &mut buffer)?);
        }
        rows.push(values);
    }

    Ok(TextQueryResult { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::{build_connection_string, connection_string_value};
    use crate::models::{ConnectionParams, DatabaseSelection, PluginSettings};

    fn params(password: &str) -> ConnectionParams {
        ConnectionParams {
            driver: "db2".to_string(),
            host: Some("localhost".to_string()),
            port: Some(50000),
            username: Some("db2inst1".to_string()),
            password: Some(password.to_string()),
            database: DatabaseSelection::Single("sample".to_string()),
            ssl_mode: None,
            connect_timeout_secs: None,
        }
    }

    #[test]
    fn builds_db2_connection_string() {
        let connection_string =
            build_connection_string(&params("secret"), &PluginSettings::default());
        assert_eq!(
            connection_string,
            "Driver={IBM DB2 ODBC DRIVER};Hostname=localhost;Port=50000;Protocol=TCPIP;\
             Database=sample;Uid=db2inst1;Pwd=secret"
        );
    }

    #[test]
    fn applies_driver_security_and_extra_settings() {
        let settings = PluginSettings {
            driver_name: Some("Db2".to_string()),
            security: Some("ssl".to_string()),
            current_schema: Some("APP".to_string()),
            extra_properties: Some("ConnectTimeout=5;QueryTimeout=30;".to_string()),
        };
        let connection_string = build_connection_string(&params("secret"), &settings);
        assert!(connection_string.starts_with("Driver={Db2};"));
        assert!(connection_string.contains(";Security=SSL;"));
        assert!(connection_string.ends_with(";ConnectTimeout=5;QueryTimeout=30"));
        // The schema is applied with SET CURRENT SCHEMA, not a keyword.
        assert!(!connection_string.contains("CurrentSchema"));
    }

    #[test]
    fn braces_values_that_would_break_the_connection_string() {
        assert_eq!(connection_string_value("plain"), "plain");
        assert_eq!(connection_string_value("a;b"), "{a;b}");
        assert_eq!(connection_string_value("a}b"), "{a}}b}");
        assert_eq!(connection_string_value(" padded "), "{ padded }");

        let connection_string =
            build_connection_string(&params("p;w}d"), &PluginSettings::default());
        assert!(connection_string.ends_with("Pwd={p;w}}d}"));
    }
}
