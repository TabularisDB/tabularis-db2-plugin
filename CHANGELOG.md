# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed

- Schema and object browsing, verified against Db2 12.1 and Tabularis 0.32.0
  (#1):
  - `get_columns`, `get_view_columns`, `get_all_columns_batch` and
    `get_schema_snapshot` no longer report the catalog's storage size as a
    length (`INTEGER` and `DATE` came back with a length of 4, which Tabularis
    showed and generated as `INTEGER(4)`). Lengths are reported for string and
    binary types only, precision and scale for `DECIMAL` and `DECFLOAT`, and
    the full declared type (`VARCHAR(50)`, `DECIMAL(10,2)`,
    `VARCHAR(16) FOR BIT DATA`, `TIMESTAMP(12)`, `CLOB(1M)`) is sent as
    `full_type`. `CHARACTER` is reported as `CHAR`, the name the type list
    uses.
  - Columns now carry `is_generated` (`GENERATED ALWAYS AS (...)` columns,
    which Tabularis leaves out of inserts) and `default_sql`; an empty
    default or comment is omitted instead of sent as an empty string.
    Implicitly hidden columns are not listed, matching what `SELECT *`
    returns.
  - `get_schemas` lists every user schema, including empty ones, and hides
    the `NULLID` and `SQLJ` system schemas along with `SYS*`.
  - `get_tables` also lists typed tables, materialized query tables, created
    temporary tables and nicknames.
  - Schemas, tables and columns with delimited names (mixed case, spaces,
    quotes, reserved words, non-ASCII letters) work: the current schema is
    set with `SET CURRENT SCHEMA '<name>'` instead of the `CurrentSchema`
    connection keyword, which folded the name to upper case.
  - Without a selected schema the plugin falls back to the user's schema in
    upper case (`DB2INST1`), as the catalog stores it, instead of the login
    name as typed.
  - `get_all_columns_batch` and `get_all_foreign_keys_batch` honour the
    `tables` filter.
- Values longer than 4,096 bytes (CLOBs, view and routine bodies) were cut
  off; they are now read in full, including multi-byte characters that
  straddle a buffer boundary.
- Error messages are the Db2 message (`SQL0204N "T" is an undefined name.
  SQLSTATE=42704`) instead of the ODBC call name and driver tags wrapped
  around it; a missing or misnamed ODBC driver gets a hint about the "ODBC
  Driver Name" setting.
- A password, user, host or database containing `;`, `{` or `}` no longer
  breaks the connection string.
- An unknown method answers `-32601` with `Method not found: <name>`, the
  text older Tabularis versions match to fall back for optional methods, and
  notifications (requests without an `id`, such as `cancel`) are no longer
  answered.

- Query execution, verified against Db2 12.1 and Tabularis 0.32.0 (#2):
  - Integers, floating-point numbers and booleans are returned as JSON
    numbers and booleans instead of strings. Tabularis reads `COUNT(*)` as
    an integer, so the row count of every grid was 0. `DECIMAL` and
    `DECFLOAT` stay text, digit for digit; a `BLOB` comes back in the host's
    blob format with its size, MIME type and first 10,240 bytes.
  - Whether a statement returns rows is decided by Db2, not by its first
    keyword: `VALUES`, `CALL` with a result set and a statement that starts
    with a comment returned nothing before.
  - Pagination no longer rewrites the query. The statement runs as written
    and the page is taken on the cursor, so `ORDER BY`, `FETCH FIRST n ROWS
    ONLY`, `WITH UR` and columns that share a name all keep working, and
    `has_more` is exact (a page that ended on the last row claimed another
    one). Without a `limit` every row is returned.
  - `affected_rows` is the real count for `INSERT`, `UPDATE`, `DELETE` and
    `MERGE` (it was always 0), and 0 for DDL.
  - An error raised while fetching (a division by zero in a row, for
    example) is reported as the Db2 error. It used to kill the plugin
    process, taking every open connection with it.
  - Non-ASCII text survives a `C` locale: the plugin pins the Db2 CLI code
    page to UTF-8 (`DB2CODEPAGE=1208`) unless it is already set. Under
    `LANG=C` — a container, a service — accented and CJK characters were
    replaced.
  - Unqualified routine names resolve in the selected schema (it is added
    to `CURRENT PATH`), like unqualified table names already did.
  - A trailing `;` is accepted, and an empty statement is rejected with a
    clear message.
  - "Set to server NOW" in the data grid works: Tabularis asks for the
    server time with `SELECT NOW()`, which Db2 rejects without a `FROM`, so
    that exact statement is answered with `VALUES CURRENT TIMESTAMP`.

### Added

- `execute_query_batch`: a script's statements run in order on one
  connection, so session state set by one statement (`SET CURRENT SCHEMA`,
  for instance) is seen by the next. Tabularis previously fell back to one
  connection per statement.
- The manifest declares `sql_dialect` (`generic`) and a `sql_syntax`
  descriptor for Db2: nested block comments, `BEGIN ... END` blocks kept as
  one statement when a script is split, the `db2` SQL formatter, and Db2
  keywords and functions for autocomplete. Without a declared dialect
  Tabularis treated the driver as PostgreSQL when writing SQL of its own and
  split compound statements at every `;`.

### Changed

- Each request now uses a single Db2 connection for all of its statements
  (`get_schema_snapshot` opened three).
- `connect_timeout_secs` from the connection settings is honoured as the
  login timeout.

## [0.0.3] - 2026-08-06

### Features

- Add plugin icon and reference it in the manifest by @debba

## [0.0.2] - 2026-07-30

### Bug Fixes

- *(manifest)* Satisfy the Tabularium driver-kind contract (#13) by @NewtTheWolf in [#13](https://github.com/TabularisDB/tabularis-db2-plugin/pull/13)
- *(ci)* Keep .tabularium when uploading the manifest artifact by @NewtTheWolf

### Documentation

- Fix Discord shields.io badge server id by @debba

### Features

- Migrate to the .tabularium manifest for the Tabularium registry by @NewtTheWolf

### Miscellaneous

- Switch Discord invite URL to discord.com/invite/K2hmhfHRSt by @debba

## [0.0.1] - 2026-04-14

### Bug Fixes

- *(metadata)* Correct foreign key joins to map referenced columns by @debba
- *(odbc)* Add platform-specific linking and update CI runners by @debba

### Features

- Add initial IBM Db2 plugin scaffold with CI and tooling by @debba
- *(test)* Add dockerized DB2 integration tests and setup scripts by @debba
- *(query)* Implement DB2 explain plan extraction and tree building by @debba

### Miscellaneous

- *(release)* Push commits to master branch instead of main by @debba
- *(release)* Update macos runner to 15 and mark explain query verified by @debba
- *(release)* Update macos runner to version 13 for x86_64 build by @debba
- *(release)* Update macos runner to macos-latest by @debba
- *(release)* Add macOS x86_64 cross-compile ODBC setup and env path by @debba
