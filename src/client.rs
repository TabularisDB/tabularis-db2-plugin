use crate::error::{clean_odbc_message, odbc_error, PluginResult};
use crate::models::{ConnectionParams, PluginSettings};
use crate::utils::blob::{encode_blob_preview, MAX_BLOB_PREVIEW_SIZE};
use crate::utils::identifiers::quote_identifier;
use crate::utils::values::string_literal;
use odbc_api::handles::{AsStatementRef, Record, Statement};
use odbc_api::parameter::InputParameter;
use odbc_api::{Connection, ConnectionOptions, Cursor, Environment, IntoParameter};
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
    fn SQLRowCount(stmt: OdbcHandle, row_count: *mut OdbcLen) -> i16;
    fn SQLSetStmtAttr(stmt: OdbcHandle, attribute: i32, value: OdbcHandle, length: i32) -> i16;
    fn SQLPrepare(stmt: OdbcHandle, text: *const u8, length: i32) -> i16;
    fn SQLColAttribute(
        stmt: OdbcHandle,
        column: u16,
        field: u16,
        character_attribute: *mut c_void,
        buffer_length: i16,
        string_length: *mut i16,
        numeric_attribute: *mut OdbcLen,
    ) -> i16;
}

const SQL_C_CHAR: i16 = 1;
const SQL_C_BINARY: i16 = -2;
const SQL_SUCCESS: i16 = 0;
const SQL_SUCCESS_WITH_INFO: i16 = 1;
const SQL_NO_DATA: i16 = 100;
const SQL_NULL_DATA: OdbcLen = -1;
const SQL_NO_TOTAL: OdbcLen = -4;
/// Db2 CLI's statement attribute for deferred prepare, and its "off" value.
const SQL_ATTR_DEFERRED_PREPARE: i32 = 1277;
const SQL_DEFERRED_PREPARE_OFF: usize = 0;

/// Size of the buffer one `SQLGetData` call fills. Longer values (CLOBs,
/// view and routine bodies, long comments) are read in several calls.
const FETCH_CHUNK_BYTES: usize = 8192;

static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();

const SQL_DESC_CONCISE_TYPE: u16 = 2;

// ODBC SQL type codes, plus the ones the Db2 CLI driver reports for its own
// types (BLOB and BOOLEAN).
const SQL_INTEGER: i32 = 4;
const SQL_SMALLINT: i32 = 5;
const SQL_FLOAT: i32 = 6;
const SQL_REAL: i32 = 7;
const SQL_DOUBLE: i32 = 8;
const SQL_BOOLEAN: i32 = 16;
const SQL_LONGVARBINARY: i32 = -4;
const SQL_BIGINT: i32 = -5;
const SQL_TINYINT: i32 = -6;
const SQL_BIT: i32 = -7;
const SQL_BLOB: i32 = -98;

/// How the values of a result column should be handed to Tabularis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// `SMALLINT`, `INTEGER`, `BIGINT`: a JSON integer.
    Integer,
    /// `REAL`, `DOUBLE`: a JSON number.
    Float,
    /// `BOOLEAN`: a JSON boolean.
    Boolean,
    /// `BLOB`: the host's `BLOB:<size>:<mime>:<base64>` wire format.
    Blob,
    /// Everything else, as the text Db2 renders: strings, dates, `DECIMAL`
    /// and `DECFLOAT` (kept as text so no digit is lost), `XML`, and the hex
    /// form of `BINARY` / `FOR BIT DATA` values.
    Text,
}

impl ColumnKind {
    fn from_sql_type(sql_type: i32) -> Self {
        match sql_type {
            SQL_SMALLINT | SQL_INTEGER | SQL_BIGINT | SQL_TINYINT => Self::Integer,
            SQL_REAL | SQL_FLOAT | SQL_DOUBLE => Self::Float,
            SQL_BOOLEAN | SQL_BIT => Self::Boolean,
            SQL_BLOB | SQL_LONGVARBINARY => Self::Blob,
            _ => Self::Text,
        }
    }
}

/// Asks the driver for the SQL type of a result column.
///
/// Goes through a raw `SQLColAttribute` call for the same reason
/// [`read_cell`] uses raw `SQLGetData`: the IBM clidriver writes a 32-bit
/// value into the 64-bit output, and `odbc-api`'s own `col_data_type` panics
/// on the garbage left in the upper half. A column whose type cannot be read
/// is treated as text, which is always a faithful rendering.
fn column_kind(statement: OdbcHandle, column: u16) -> ColumnKind {
    let mut sql_type: OdbcLen = 0;
    let ret = unsafe {
        SQLColAttribute(
            statement,
            column,
            SQL_DESC_CONCISE_TYPE,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut sql_type,
        )
    };
    if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
        return ColumnKind::Text;
    }
    ColumnKind::from_sql_type(sql_type as i32)
}

/// A result set fetched as text: one `Option<String>` per cell, `None` for
/// SQL NULL. `kinds` says how each column's text is meant to be read.
#[derive(Debug, Default)]
pub struct TextQueryResult {
    pub columns: Vec<String>,
    pub kinds: Vec<ColumnKind>,
    pub rows: Vec<Vec<Option<String>>>,
    /// True when a row window was requested and rows exist beyond it.
    pub has_more: bool,
}

/// The slice of a result set a caller wants: skip `offset` rows, then read
/// at most `limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub offset: u64,
    pub limit: u64,
}

/// A value bound to a `?` parameter marker.
///
/// Text and binary data travel as parameters rather than as SQL literals: a
/// string constant is limited to 32,672 bytes, which a CLOB or a BLOB easily
/// exceeds, and a parameter needs no quoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    Text(String),
    Bytes(Vec<u8>),
}

impl Param {
    fn boxed(&self) -> Box<dyn InputParameter> {
        match self {
            Self::Text(text) => Box::new(text.clone().into_parameter()),
            Self::Bytes(bytes) => Box::new(bytes.clone().into_parameter()),
        }
    }
}

fn boxed_params(params: &[Param]) -> Vec<Box<dyn InputParameter>> {
    params.iter().map(Param::boxed).collect()
}

/// What running a statement produced.
#[derive(Debug)]
pub enum Outcome {
    /// The statement returned a result set (a query, `VALUES`, a `CALL` that
    /// opens a cursor).
    Rows(TextQueryResult),
    /// The statement returned none; the number of rows it changed.
    Affected(u64),
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
            // Unqualified function and procedure names are resolved through
            // CURRENT PATH, not CURRENT SCHEMA. Adding the schema to the path
            // lets `CALL MY_PROC()` and `MY_FUNC(x)` find the routines of the
            // schema the user is working in. Best effort: Db2 rejects a path
            // that would name a schema twice (the user's own schema is
            // already on it), and the path is then right as it is.
            let _ = session.execute(&format!(
                "SET CURRENT PATH = CURRENT PATH, {}",
                quote_identifier(schema)
            ));
        }

        Ok(session)
    }

    /// Runs a statement that must produce a result set and fetches all of it.
    pub fn query(&self, sql: &str) -> PluginResult<TextQueryResult> {
        match self.run(sql, None)? {
            Outcome::Rows(result) => Ok(result),
            Outcome::Affected(_) => Err("The query did not return a result set".to_string()),
        }
    }

    /// Runs a statement for its side effect and returns the number of rows it
    /// changed (0 when Db2 reports none, e.g. for DDL).
    pub fn execute(&self, sql: &str) -> PluginResult<u64> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        // A statement that happens to return rows (a CALL with a result set)
        // is still a success; the cursor is released unread.
        if let Some(cursor) = statement.execute(sql, ()).map_err(odbc_error)? {
            release_cursor(cursor);
            return Ok(0);
        }
        Ok(affected_rows(&mut statement))
    }

    /// Like [`Self::execute`], with values bound to the statement's `?`
    /// markers in order.
    pub fn execute_with(&self, sql: &str, params: &[Param]) -> PluginResult<u64> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        let bound = boxed_params(params);
        if let Some(cursor) = statement
            .execute(sql, bound.as_slice())
            .map_err(odbc_error)?
        {
            release_cursor(cursor);
            return Ok(0);
        }
        Ok(affected_rows(&mut statement))
    }

    /// Runs a query with bound parameters and returns the first column of
    /// its first row as raw bytes, read in full; `None` when the query
    /// returns no row or the value is NULL.
    pub fn fetch_bytes(&self, sql: &str, params: &[Param]) -> PluginResult<Option<Vec<u8>>> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        let bound = boxed_params(params);
        let Some(mut cursor) = statement
            .execute(sql, bound.as_slice())
            .map_err(odbc_error)?
        else {
            return Err("The query did not return a result set".to_string());
        };
        let result = match fetch_next(&mut cursor) {
            Ok(true) => read_binary_cell(&mut cursor, 1),
            Ok(false) => Ok(None),
            Err(error) => Err(error),
        };
        release_cursor(cursor);
        result
    }

    /// Runs any statement and reports what it produced. Whether it is a query
    /// is decided by Db2 (did it open a result set?), not by looking at the
    /// SQL text, so `VALUES`, `CALL`, a `WITH` that ends in an `INSERT` and a
    /// statement that starts with a comment are all classified correctly.
    ///
    /// `window` pages the result set on the cursor: the statement runs as
    /// written and rows outside the window are skipped or left unread. That
    /// keeps the user's `ORDER BY`, `FETCH FIRST` and isolation clauses
    /// intact, which wrapping the text in another `SELECT` would not.
    pub fn run(&self, sql: &str, window: Option<Window>) -> PluginResult<Outcome> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        if let Some(mut cursor) = statement.execute(sql, ()).map_err(odbc_error)? {
            let result = fetch_text_rows(&mut cursor, window);
            release_cursor(cursor);
            return result.map(Outcome::Rows);
        }
        Ok(Outcome::Affected(affected_rows(&mut statement)))
    }
}

impl Session {
    /// Has Db2 compile a statement without running it, and returns the
    /// error it reports. CLI defers a prepare until the statement runs,
    /// answering describe calls on its own, so deferral is turned off for
    /// this statement.
    pub fn check(&self, sql: &str) -> PluginResult<()> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        let handle = statement.as_stmt_ref().as_sys().0 as OdbcHandle;
        let ret = unsafe {
            SQLSetStmtAttr(
                handle,
                SQL_ATTR_DEFERRED_PREPARE,
                SQL_DEFERRED_PREPARE_OFF as OdbcHandle,
                0,
            )
        };
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(statement_error(&mut statement, "SQLSetStmtAttr", ret));
        }
        let ret = unsafe { SQLPrepare(handle, sql.as_ptr(), sql.len() as i32) };
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(statement_error(&mut statement, "SQLPrepare", ret));
        }
        Ok(())
    }

    /// Runs a statement to the end and says how many rows it returned, or
    /// changed when it returns none. The rows are fetched but their values
    /// are never read: this is for timing a statement, not for its data.
    pub fn drain(&self, sql: &str) -> PluginResult<u64> {
        let mut statement = self.connection.preallocate().map_err(odbc_error)?;
        if let Some(mut cursor) = statement.execute(sql, ()).map_err(odbc_error)? {
            let mut count = 0u64;
            let result = loop {
                match fetch_next(&mut cursor) {
                    Ok(true) => count += 1,
                    Ok(false) => break Ok(count),
                    Err(error) => break Err(error),
                }
            };
            release_cursor(cursor);
            return result;
        }
        Ok(affected_rows(&mut statement))
    }
}

/// Lets go of a cursor without closing it.
///
/// Dropping an `odbc-api` cursor calls `SQLCloseCursor` and panics when that
/// fails — which it does whenever Db2 already closed the cursor itself, as
/// it does after an error during a fetch (a division by zero in a row, a
/// value that cannot be converted). That turned a query error into a dead
/// plugin process. Every cursor here borrows a preallocated statement, and
/// freeing that statement handle closes whatever cursor is still open on it,
/// so nothing is leaked by skipping the explicit close.
fn release_cursor(cursor: impl Cursor) {
    std::mem::forget(cursor);
}

/// The message Db2 left on a statement handle for the call that just failed.
fn statement_error(statement: &mut impl AsStatementRef, call: &str, ret: i16) -> String {
    let handle = statement.as_stmt_ref();
    let mut record = Record::with_capacity(512);
    if record.fill_from(&handle, 1) {
        clean_odbc_message(&record.to_string())
    } else {
        format!("{call} failed with return code {ret}")
    }
}

/// Number of rows the last statement on `statement` changed; 0 when Db2
/// reports none (DDL, `SET`, a compound statement).
///
/// Raw `SQLRowCount`, read back as an `i32`: the IBM clidriver writes the
/// count as a 32-bit value, so its "not available" answer of -1 would
/// otherwise read as 4,294,967,295 rows.
fn affected_rows(statement: &mut impl AsStatementRef) -> u64 {
    let handle = statement.as_stmt_ref().as_sys().0 as OdbcHandle;
    let mut count: OdbcLen = 0;
    let ret = unsafe { SQLRowCount(handle, &mut count) };
    if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
        return 0;
    }
    (count as i32).max(0) as u64
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
    cursor: &mut impl Cursor,
    column: u16,
    buffer: &mut [u8],
) -> PluginResult<Option<String>> {
    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
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
            return Err(statement_error(cursor, "SQLGetData", ret));
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

/// Reads the start of a `BLOB` cell and its total size, and renders them in
/// the host's blob wire format.
///
/// Only the first [`MAX_BLOB_PREVIEW_SIZE`] bytes are transferred: a grid
/// page of large objects would otherwise pull every one of them in full. The
/// driver reports the full length on the first call, so the header still
/// carries the true size.
fn read_blob_cell(cursor: &mut impl Cursor, column: u16) -> PluginResult<Option<String>> {
    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
    let mut buffer = vec![0u8; MAX_BLOB_PREVIEW_SIZE];
    let mut indicator: OdbcLen = 0;
    let ret = unsafe {
        SQLGetData(
            statement,
            column,
            SQL_C_BINARY,
            buffer.as_mut_ptr() as *mut c_void,
            buffer.len() as OdbcLen,
            &mut indicator,
        )
    };
    if ret == SQL_NO_DATA {
        return Ok(Some(encode_blob_preview(&[], 0)));
    }
    if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
        return Err(statement_error(cursor, "SQLGetData", ret));
    }

    let indicator = (indicator as i32) as OdbcLen;
    if indicator == SQL_NULL_DATA {
        return Ok(None);
    }

    // SQL_NO_TOTAL: the driver cannot say how long the value is; all that is
    // known is that it filled the buffer.
    let total = if indicator < 0 {
        buffer.len()
    } else {
        indicator as usize
    };
    let preview = &buffer[..total.min(buffer.len())];
    Ok(Some(encode_blob_preview(preview, total)))
}

/// Reads one cell as raw bytes, however long it is (see [`read_cell`] for
/// why this goes through raw `SQLGetData` calls).
fn read_binary_cell(cursor: &mut impl Cursor, column: u16) -> PluginResult<Option<Vec<u8>>> {
    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut bytes: Vec<u8> = Vec::new();

    loop {
        let mut indicator: OdbcLen = 0;
        let ret = unsafe {
            SQLGetData(
                statement,
                column,
                SQL_C_BINARY,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as OdbcLen,
                &mut indicator,
            )
        };
        if ret == SQL_NO_DATA {
            break;
        }
        if ret != SQL_SUCCESS && ret != SQL_SUCCESS_WITH_INFO {
            return Err(statement_error(cursor, "SQLGetData", ret));
        }

        let indicator = (indicator as i32) as OdbcLen;
        if indicator == SQL_NULL_DATA {
            return Ok(None);
        }

        // Binary data is not NUL-terminated: a full buffer is a full piece.
        let truncated = ret == SQL_SUCCESS_WITH_INFO
            && (indicator == SQL_NO_TOTAL || indicator as usize > buffer.len());
        let piece = if truncated {
            buffer.len()
        } else {
            (indicator.max(0) as usize).min(buffer.len())
        };
        bytes.extend_from_slice(&buffer[..piece]);

        if !truncated {
            break;
        }
    }

    Ok(Some(bytes))
}

/// Advances the cursor one row; `false` at the end of the result set. An
/// error raised while Db2 computes the row (a division by zero, an overflow)
/// surfaces here, with Db2's own message.
fn fetch_next(cursor: &mut impl Cursor) -> PluginResult<bool> {
    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
    match unsafe { SQLFetch(statement) } {
        SQL_NO_DATA => Ok(false),
        SQL_SUCCESS | SQL_SUCCESS_WITH_INFO => Ok(true),
        ret => Err(statement_error(cursor, "SQLFetch", ret)),
    }
}

/// Fetches a result set as text. With a `window`, the first `offset` rows are
/// skipped without reading their values and at most `limit` rows are read;
/// one more fetch tells whether rows remain.
fn fetch_text_rows(
    cursor: &mut impl Cursor,
    window: Option<Window>,
) -> PluginResult<TextQueryResult> {
    let column_count = cursor.num_result_cols().map_err(odbc_error)? as u16;
    let columns = cursor
        .column_names()
        .map_err(odbc_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(odbc_error)?;

    let statement = cursor.as_stmt_ref().as_sys().0 as OdbcHandle;
    let kinds = (1..=column_count)
        .map(|column| column_kind(statement, column))
        .collect::<Vec<_>>();

    let mut result = TextQueryResult {
        columns,
        kinds,
        rows: Vec::new(),
        has_more: false,
    };

    if let Some(window) = window {
        for _ in 0..window.offset {
            if !fetch_next(cursor)? {
                return Ok(result);
            }
        }
    }

    let mut buffer = vec![0u8; FETCH_CHUNK_BYTES];
    loop {
        if !fetch_next(cursor)? {
            break;
        }
        if window.is_some_and(|window| result.rows.len() as u64 >= window.limit) {
            result.has_more = true;
            break;
        }

        let mut values = Vec::with_capacity(column_count as usize);
        for column in 1..=column_count {
            let value = match result.kinds[(column - 1) as usize] {
                ColumnKind::Blob => read_blob_cell(cursor, column)?,
                _ => read_cell(cursor, column, &mut buffer)?,
            };
            values.push(value);
        }
        result.rows.push(values);
    }

    Ok(result)
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
    fn maps_sql_types_to_column_kinds() {
        use super::ColumnKind;
        for (sql_type, kind) in [
            (5, ColumnKind::Integer),  // SMALLINT
            (4, ColumnKind::Integer),  // INTEGER
            (-5, ColumnKind::Integer), // BIGINT
            (7, ColumnKind::Float),    // REAL
            (8, ColumnKind::Float),    // DOUBLE
            (16, ColumnKind::Boolean), // BOOLEAN
            (-98, ColumnKind::Blob),   // BLOB
            (3, ColumnKind::Text),     // DECIMAL: text, to keep every digit
            (-360, ColumnKind::Text),  // DECFLOAT
            (12, ColumnKind::Text),    // VARCHAR
            (-99, ColumnKind::Text),   // CLOB
            (-3, ColumnKind::Text),    // VARBINARY: hex text
            (93, ColumnKind::Text),    // TIMESTAMP
            (-370, ColumnKind::Text),  // XML
        ] {
            assert_eq!(ColumnKind::from_sql_type(sql_type), kind, "{sql_type}");
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
