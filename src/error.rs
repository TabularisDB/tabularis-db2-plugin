pub type PluginResult<T> = Result<T, String>;

/// Turns an ODBC error into the message Tabularis shows to the user.
pub fn odbc_error(error: odbc_api::Error) -> String {
    clean_odbc_message(&error.to_string())
}

/// Reduces an `odbc-api` error to what Db2 actually said.
///
/// `odbc-api` renders a diagnostic record as
/// `ODBC emitted an error calling 'SQLExecDirect':\nState: 42S02, Native
/// error: -204, Message: [IBM][CLI Driver][DB2/LINUXX8664] SQL0204N  "T" is
/// an undefined name.  SQLSTATE=42704`. The ODBC call name and the driver
/// component tags say nothing about the user's SQL, so only the message is
/// kept (`SQL0204N "T" is an undefined name. SQLSTATE=42704`). A message
/// without a SQLSTATE of its own gets the record's state appended, and the
/// driver-manager errors that mean "no usable ODBC driver" get a hint about
/// the plugin setting that fixes them.
pub fn clean_odbc_message(raw: &str) -> String {
    let state = raw
        .split("State: ")
        .nth(1)
        .and_then(|rest| rest.split([',', '\n']).next())
        .map(str::trim)
        .filter(|state| !state.is_empty() && *state != "00000");

    let Some(message) = raw.split("Message: ").nth(1) else {
        return collapse_whitespace(raw);
    };

    let mut message = collapse_whitespace(strip_component_tags(message));

    if let Some(state) = state {
        if !message.contains("SQLSTATE") {
            message.push_str(&format!(" SQLSTATE={state}"));
        }
    }

    if let Some(hint) = reorg_hint(&message) {
        message.push(' ');
        message.push_str(&hint);
    }

    if is_missing_driver(&message) {
        message.push_str(
            " Check that the IBM Db2 ODBC driver is installed and that the plugin's \
             \"ODBC Driver Name\" setting matches its registered name.",
        );
    }

    message
}

/// Drops the leading `[IBM][CLI Driver][DB2/LINUXX8664]`-style tags.
fn strip_component_tags(message: &str) -> &str {
    let mut rest = message.trim_start();
    while rest.starts_with('[') {
        match rest.find(']') {
            Some(end) => rest = rest[end + 1..].trim_start(),
            None => break,
        }
    }
    rest
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What to do about SQL0668N with reason code 7: the table is in "reorg
/// pending" state, which is where an `ALTER TABLE` that drops a column or
/// changes a column's type or nullability leaves it. Db2 says the operation
/// is not allowed and names the table; the fix is one statement away.
fn reorg_hint(message: &str) -> Option<String> {
    if !message.starts_with("SQL0668N") || !message.contains("reason code \"7\"") {
        return None;
    }
    let table = message.split("on table \"").nth(1)?.split('"').next()?;
    let (schema, name) = table.split_once('.')?;
    Some(format!(
        "The table must be reorganized after the last ALTER TABLE: run CALL \
         SYSPROC.ADMIN_CMD('REORG TABLE \"{schema}\".\"{name}\"')."
    ))
}

fn is_missing_driver(message: &str) -> bool {
    message.contains("Data source name not found")
        || message.contains("Can't open lib")
        || message.contains("SQLSTATE=IM002")
        || message.contains("SQLSTATE=IM003")
}

#[cfg(test)]
mod tests {
    use super::clean_odbc_message;

    #[test]
    fn keeps_only_the_db2_message() {
        let raw =
            "ODBC emitted an error calling 'SQLExecDirect':\nState: 42S02, Native error: -204, \
                   Message: [IBM][CLI Driver][DB2/LINUXX8664] SQL0204N  \"TEST_SCHEMA.NOPE\" is an \
                   undefined name.  SQLSTATE=42704\n";
        assert_eq!(
            clean_odbc_message(raw),
            "SQL0204N \"TEST_SCHEMA.NOPE\" is an undefined name. SQLSTATE=42704"
        );
    }

    #[test]
    fn appends_the_state_when_the_message_has_none() {
        let raw = "ODBC emitted an error calling 'SQLDriverConnect':\nState: 08001, Native error: \
                   -30081, Message: [IBM][CLI Driver] SQL30081N  A communication error has been \
                   detected.";
        assert_eq!(
            clean_odbc_message(raw),
            "SQL30081N A communication error has been detected. SQLSTATE=08001"
        );
    }

    #[test]
    fn hints_at_the_driver_setting_when_the_driver_cannot_be_loaded() {
        let raw = "ODBC emitted an error calling 'SQLDriverConnect':\nState: 01000, Native error: 0, \
                   Message: [unixODBC][Driver Manager]Can't open lib '/opt/ibm/clidriver/lib/libdb2.so' \
                   : file not found";
        let message = clean_odbc_message(raw);
        assert!(message.starts_with("Can't open lib '/opt/ibm/clidriver/lib/libdb2.so'"));
        assert!(message.contains("ODBC Driver Name"));

        let raw =
            "ODBC emitted an error calling 'SQLDriverConnect':\nState: IM002, Native error: 0, \
                   Message: [unixODBC][Driver Manager]Data source name not found and no default \
                   driver specified";
        assert!(clean_odbc_message(raw).contains("ODBC Driver Name"));
    }

    #[test]
    fn says_how_to_leave_reorg_pending_state() {
        let raw =
            "ODBC emitted an error calling 'SQLExecDirect':\nState: 57016, Native error: -668, \
                   Message: [IBM][CLI Driver][DB2/LINUXX8664] SQL0668N  Operation not allowed for \
                   reason code \"7\" on table \"TEST_SCHEMA.DDL_PROBE\".  SQLSTATE=57007";
        assert_eq!(
            clean_odbc_message(raw),
            "SQL0668N Operation not allowed for reason code \"7\" on table \
             \"TEST_SCHEMA.DDL_PROBE\". SQLSTATE=57007 The table must be reorganized after the \
             last ALTER TABLE: run CALL SYSPROC.ADMIN_CMD('REORG TABLE \
             \"TEST_SCHEMA\".\"DDL_PROBE\"')."
        );
        // Another reason code is another problem.
        let other = raw.replace("reason code \"7\"", "reason code \"1\"");
        assert!(!clean_odbc_message(&other).contains("REORG"));
    }

    #[test]
    fn passes_through_text_that_is_not_a_diagnostic_record() {
        assert_eq!(
            clean_odbc_message("No Diagnostics available.\n  Sadly."),
            "No Diagnostics available. Sadly."
        );
    }
}
