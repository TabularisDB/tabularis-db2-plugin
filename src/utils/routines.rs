//! How `SYSCAT.ROUTINES` and `SYSCAT.ROUTINEPARMS` describe a routine, in
//! the words Tabularis uses.

/// `ROUTINETYPE` as the routine type Tabularis groups the sidebar by.
/// Methods (`M`) belong to a structured type and are not listed.
pub fn routine_type(code: &str) -> Option<&'static str> {
    match code.trim() {
        "F" => Some("FUNCTION"),
        "P" => Some("PROCEDURE"),
        _ => None,
    }
}

/// What a `ROUTINEPARMS` row stands for.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ParameterRole {
    /// An argument of the routine, with its direction.
    Argument(&'static str),
    /// The value a scalar function returns: it has no name.
    ReturnValue,
    /// A column of the table a table function returns. These are numbered
    /// on their own, starting again from 1.
    ResultColumn,
}

/// Reads the role of a parameter row from its `ROWTYPE` and `ORDINAL`.
///
/// `C` is the result of a function "after casting" and `R` the result
/// "before casting": a function Db2 ships records its return value as `R`,
/// one a user created as `C`, and both sit at position 0. The columns of a
/// table function are `R` rows from position 1 on.
pub fn parameter_role(row_type: &str, ordinal: i64) -> Option<ParameterRole> {
    match row_type.trim() {
        "P" => Some(ParameterRole::Argument("IN")),
        "O" => Some(ParameterRole::Argument("OUT")),
        "B" => Some(ParameterRole::Argument("INOUT")),
        "C" => Some(ParameterRole::ReturnValue),
        "R" if ordinal == 0 => Some(ParameterRole::ReturnValue),
        "R" => Some(ParameterRole::ResultColumn),
        _ => None,
    }
}

/// The argument types of a routine, as written between the parentheses of
/// `DROP FUNCTION name(...)`. Db2 lets routines share a name, so this is
/// what tells two of them apart; a routine without arguments has the empty
/// signature.
pub fn signature<S: AsRef<str>>(argument_types: &[S]) -> String {
    argument_types
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `NAME(signature)`, the way a routine is named in messages.
pub fn routine_label(name: &str, signature: &str) -> String {
    format!("{name}({signature})")
}

#[cfg(test)]
mod tests {
    use super::{parameter_role, routine_label, routine_type, signature, ParameterRole};

    #[test]
    fn routine_types_are_spelled_out() {
        assert_eq!(routine_type("F"), Some("FUNCTION"));
        assert_eq!(routine_type("P"), Some("PROCEDURE"));
        assert_eq!(routine_type("P "), Some("PROCEDURE"));
        assert_eq!(routine_type("M"), None);
        assert_eq!(routine_type(""), None);
    }

    #[test]
    fn row_types_map_to_directions() {
        assert_eq!(parameter_role("P", 1), Some(ParameterRole::Argument("IN")));
        assert_eq!(parameter_role("O", 2), Some(ParameterRole::Argument("OUT")));
        assert_eq!(
            parameter_role("B", 3),
            Some(ParameterRole::Argument("INOUT"))
        );
        assert_eq!(parameter_role("S", 1), None);
    }

    #[test]
    fn results_are_told_from_arguments() {
        // A function created by a user, then one Db2 ships.
        assert_eq!(parameter_role("C", 0), Some(ParameterRole::ReturnValue));
        assert_eq!(parameter_role("R", 0), Some(ParameterRole::ReturnValue));
        // The columns of a table function.
        assert_eq!(parameter_role("R", 1), Some(ParameterRole::ResultColumn));
        assert_eq!(parameter_role("R", 2), Some(ParameterRole::ResultColumn));
    }

    #[test]
    fn signature_lists_the_argument_types() {
        assert_eq!(signature::<&str>(&[]), "");
        assert_eq!(signature(&["INTEGER"]), "INTEGER");
        assert_eq!(
            signature(&["VARCHAR(20)".to_string(), "DECIMAL(9,2)".to_string()]),
            "VARCHAR(20), DECIMAL(9,2)"
        );
    }

    #[test]
    fn label_keeps_the_empty_signature() {
        assert_eq!(routine_label("NOW_UTC", ""), "NOW_UTC()");
        assert_eq!(routine_label("F", "INTEGER"), "F(INTEGER)");
    }
}
