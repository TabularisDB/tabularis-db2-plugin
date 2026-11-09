use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;

/// A JSON-RPC error response: the code tells Tabularis what kind of failure
/// it is, the message is what the user reads.
#[derive(Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

pub type RpcResult = Result<Value, RpcError>;

impl RpcError {
    /// The request is missing an argument or carries one of the wrong shape.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    /// Tabularis falls back to its own behaviour for optional methods when it
    /// sees this code, and older hosts when the message says so.
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
        }
    }

    /// Db2 (or the ODBC driver) rejected the operation.
    pub fn database(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }
}

pub fn send_success(
    writer: &mut impl Write,
    id: Value,
    result: impl Serialize,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id,
    });
    writeln!(writer, "{payload}").map_err(|error| error.to_string())
}

pub fn send_error(
    writer: &mut impl Write,
    id: Value,
    code: i32,
    message: &str,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "error": {
            "code": code,
            "message": message
        },
        "id": id,
    });
    writeln!(writer, "{payload}").map_err(|error| error.to_string())
}
