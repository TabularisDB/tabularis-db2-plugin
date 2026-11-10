//! Reading the text Db2 keeps for a view.
//!
//! `SYSCAT.VIEWS.TEXT` is the whole statement as it was typed
//! (`CREATE VIEW S.V (A, B) AS SELECT ...`). Tabularis wants the query on
//! its own: it shows it in the view editor and sends it back to
//! `alter_view`, which wraps it in a `CREATE OR REPLACE VIEW` again.

/// The parts of a `CREATE VIEW` statement.
#[derive(Debug, PartialEq, Eq)]
pub struct ViewText<'a> {
    /// The explicit column list, parentheses included, when the view has
    /// one: `(A, B)`.
    pub column_list: Option<&'a str>,
    /// Everything after `AS`: the view's query.
    pub query: &'a str,
}

/// Length of the whitespace and comments at the start of `text`.
fn skip_blank(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut position = 0;
    loop {
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        if text[position..].starts_with("--") {
            position += text[position..].find('\n').unwrap_or(text.len() - position);
        } else if text[position..].starts_with("/*") {
            match text[position..].find("*/") {
                Some(end) => position += end + 2,
                None => return text.len(),
            }
        } else {
            return position;
        }
    }
}

/// Consumes `keyword` (case-insensitive, as a whole word) and the blank
/// space after it.
fn take_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let head = text.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &text[keyword.len()..];
    let boundary = rest
        .chars()
        .next()
        .is_none_or(|next| !(next.is_alphanumeric() || next == '_'));
    boundary.then(|| &rest[skip_blank(rest)..])
}

/// Consumes one identifier, delimited (`"My View"`, with `""` for a quote)
/// or ordinary.
fn take_identifier(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix('"') {
        let bytes = rest.as_bytes();
        let mut position = 0;
        while position < bytes.len() {
            if bytes[position] == b'"' {
                if bytes.get(position + 1) == Some(&b'"') {
                    position += 2;
                    continue;
                }
                return Some(&rest[position + 1..]);
            }
            position += 1;
        }
        None
    } else {
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '#' | '@')))
            .unwrap_or(text.len());
        (end > 0).then(|| &text[end..])
    }
}

/// Consumes a balanced `( ... )` group and returns it with what follows.
fn take_parenthesized(text: &str) -> Option<(&str, &str)> {
    if !text.starts_with('(') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut position = 0;
    while position < bytes.len() {
        match bytes[position] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[..=position], &text[position + 1..]));
                }
            }
            b'"' => {
                position += 1 + text[position + 1..].find('"')?;
            }
            _ => {}
        }
        position += 1;
    }
    None
}

/// Splits a `CREATE [OR REPLACE] VIEW name [(columns)] AS query` statement.
/// `None` when the text does not have that shape.
pub fn parse_view_text(text: &str) -> Option<ViewText<'_>> {
    let mut rest = &text[skip_blank(text)..];
    rest = take_keyword(rest, "CREATE")?;
    if let Some(after_or) = take_keyword(rest, "OR") {
        rest = take_keyword(after_or, "REPLACE")?;
    }
    rest = take_keyword(rest, "VIEW")?;

    // schema.name, either part delimited or not.
    rest = take_identifier(rest)?;
    rest = &rest[skip_blank(rest)..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        rest = take_identifier(&after_dot[skip_blank(after_dot)..])?;
        rest = &rest[skip_blank(rest)..];
    }

    let mut column_list = None;
    if let Some((list, after_list)) = take_parenthesized(rest) {
        column_list = Some(list);
        rest = &after_list[skip_blank(after_list)..];
    }

    let query = take_keyword(rest, "AS")?;
    let query = query.trim().trim_end_matches(';').trim_end();
    (!query.is_empty()).then_some(ViewText { column_list, query })
}

/// The query of a view: its catalog text without the `CREATE VIEW ... AS`
/// in front. Text that is not a `CREATE VIEW` statement is returned whole.
pub fn view_query(text: &str) -> &str {
    parse_view_text(text).map_or(text.trim(), |view| view.query)
}

/// True when a definition typed by the user is already a complete
/// `CREATE VIEW` statement rather than just the query.
pub fn is_create_view_statement(definition: &str) -> bool {
    parse_view_text(definition).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_create_view_header() {
        assert_eq!(
            view_query(
                "CREATE VIEW TEST_SCHEMA.V_EMP AS\n    SELECT e.EMP_ID\n    FROM EMPLOYEES e"
            ),
            "SELECT e.EMP_ID\n    FROM EMPLOYEES e"
        );
        assert_eq!(
            view_query("create or replace view v as select 1 from sysibm.sysdummy1;"),
            "select 1 from sysibm.sysdummy1"
        );
        assert_eq!(
            view_query("CREATE VIEW V AS WITH T AS (SELECT 1 AS A FROM X) SELECT A FROM T"),
            "WITH T AS (SELECT 1 AS A FROM X) SELECT A FROM T"
        );
    }

    #[test]
    fn handles_delimited_names_and_comments() {
        assert_eq!(
            view_query(
                "CREATE VIEW \"Mixed Case\".\"Cheap \"\"Orders\"\"\" AS SELECT \"Order Id\" FROM T"
            ),
            "SELECT \"Order Id\" FROM T"
        );
        // A name that contains the word AS, and comments before the query.
        assert_eq!(
            view_query(
                "-- nightly\nCREATE /* x */ VIEW S.\"AS\" -- why\n AS SELECT 1 AS \"AS\" FROM T"
            ),
            "SELECT 1 AS \"AS\" FROM T"
        );
        assert_eq!(
            view_query("CREATE VIEW S . V AS SELECT 1 FROM T"),
            "SELECT 1 FROM T"
        );
    }

    #[test]
    fn keeps_the_column_list_apart() {
        let view = parse_view_text("CREATE VIEW S.V (\"First Col\", B) AS SELECT X, (Y) FROM T")
            .expect("view text");
        assert_eq!(view.column_list, Some("(\"First Col\", B)"));
        assert_eq!(view.query, "SELECT X, (Y) FROM T");

        let plain = parse_view_text("CREATE VIEW S.V AS (SELECT X FROM T)").expect("view text");
        assert_eq!(plain.column_list, None);
        assert_eq!(plain.query, "(SELECT X FROM T)");
    }

    #[test]
    fn text_that_is_not_a_create_view_is_left_alone() {
        assert_eq!(view_query("  SELECT 1 FROM T  "), "SELECT 1 FROM T");
        assert_eq!(
            view_query("CREATE TABLE T (A INT)"),
            "CREATE TABLE T (A INT)"
        );
        assert_eq!(view_query("CREATE VIEW V"), "CREATE VIEW V");
        assert_eq!(view_query(""), "");
        assert!(!is_create_view_statement("SELECT CREATE_VIEW FROM T"));
        assert!(!is_create_view_statement("CREATE VIEWS AS SELECT 1 FROM T"));
        assert!(is_create_view_statement(
            "CREATE OR REPLACE VIEW V AS SELECT 1 FROM T"
        ));
    }
}
