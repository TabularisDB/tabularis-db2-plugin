# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.1.1] - 2026-11-10

### Added

- The EXPLAIN parser accepts single-statement `db2exfmt` reports, while continuing to support the plugin JSON payload.

## [0.1.0] - 2026-11-10

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

- Inserting records, verified against Db2 12.1 and Tabularis 0.32.0 (#3):
  - Values are written for the type of their column instead of as quoted
    or bare text taken from the JSON type. A number typed in the grid
    (which arrives as a string) is validated and written as a number, and
    anything that is not a number is refused with `'abc' is not a valid
    number for column "ID"` before it reaches Db2. Booleans accept the
    usual spellings; a cleared numeric, date or boolean cell is `NULL`.
  - Text and binary data are bound as parameters, so a CLOB longer than
    the 32,672 bytes a string constant may hold can be inserted, and no
    value is ever spliced into the statement.
  - BLOBs work: an inline value, a file picked or uploaded in Tabularis
    (read from disk, within the host's size limit) and hexadecimal text for
    `BINARY`, `VARBINARY` and `FOR BIT DATA` columns. A truncated preview
    of a large BLOB is refused rather than written back over the value.
  - "Use the default" inserts `DEFAULT`, and a row with no values at all is
    inserted from the column defaults.
  - The number of inserted rows is returned (it was always 0).
  - A `NOT NULL` violation names the column: Db2 reports it as
    `"TBSPACEID=2, TABLEID=9, COLNO=0"`.

- Updating records, verified against Db2 12.1 and Tabularis 0.32.0 (#4):
  - `update_record` reads the row's key from `pk_map`, which is what
    Tabularis has sent since 0.13.4. The plugin still read the older
    `pk_col` / `pk_val` pair, found nothing and generated
    `UPDATE ... WHERE "" = ...` (`SQL0104N`), so no cell could be edited.
    The old pair is still accepted.
  - Composite primary keys work: every key column is part of the match.
  - Tables without a primary key can be edited. Tabularis sends every
    column of the row; `NULL`s are matched with `IS NULL`, LOB and XML
    columns (which Db2 cannot compare) are left out, and when several rows
    are identical exactly one of them is updated.
  - With no key values at all the request fails with `Cannot identify the
    row in "S"."T"` instead of running invalid SQL, and a key that matches
    no row reports that the row was changed or deleted instead of doing
    nothing silently.
  - The new value is written for its column type like an inserted one
    (numbers validated, text and BLOBs bound, `NULL`, `DEFAULT`), and the
    number of rows changed is returned.

- Deleting records, verified against Db2 12.1 and Tabularis 0.32.0 (#5):
  - `delete_record` reads the row's key from `pk_map`, like
    `update_record`: it generated `DELETE ... WHERE "" = ...` (`SQL0104N`)
    and no row could be deleted from the grid.
  - Composite keys and tables without a primary key work as for updates,
    and exactly one row is deleted when several are identical.
  - No key values, or a key that matches no row, is an error with a clear
    message instead of invalid SQL or a silent no-op.
  - A delete stopped by a foreign key reports Db2's message, which names
    the relationship (`SQL0532N ... "TEST_SCHEMA.FK_CHILD.FK_CHILD_PARENT"
    restricts the deletion`).
  - The number of rows deleted is returned.

- DDL generation, verified by running every generated statement on Db2
  12.1 and from Tabularis 0.32.0's table designer (#6):
  - Identifiers are quoted everywhere. `get_create_table_sql` and
    `get_add_column_sql` wrote column names bare, so a name with a space,
    in mixed case or equal to a reserved word produced invalid SQL or a
    column with another name.
  - Auto-increment columns are created as `GENERATED BY DEFAULT AS
    IDENTITY` (the flag was ignored); primary-key columns are always
    `NOT NULL`, as Db2 requires.
  - `get_add_column_sql` adds `WITH DEFAULT` to a `NOT NULL` column that
    has no default: Db2 rejects the bare form on an existing table
    (`SQL0193N`).
  - `get_alter_column_sql` only handled a rename and a type change, and
    ran `SET DATA TYPE` whenever the nullability or the default differed.
    It now emits one statement per attribute that changed (rename, type,
    `SET` / `DROP NOT NULL`, `SET` / `DROP DEFAULT`, identity on or off,
    comment) and nothing when nothing changed (it returned a comment line,
    which Db2 refuses to run).
  - A change that leaves the table in "reorg pending" state is followed by
    `CALL SYSPROC.ADMIN_CMD('REORG TABLE ...')`; without it the table
    rejected every statement with `SQL0668N`.
  - Index names are qualified with the table's schema.
  - Foreign keys: the constraint, table and column names are quoted, and a
    referential action Db2 does not implement (`ON UPDATE CASCADE`,
    `SET DEFAULT`) is reported instead of generating SQL that cannot run.
  - The type list no longer offers `JSON`, which is not a Db2 column type,
    and offers `BOOLEAN`; `SMALLINT`, `INTEGER` and `BIGINT` can be
    auto-increment; `DECFLOAT` no longer asks for a scale.
- View support, verified against Db2 12.1 and Tabularis 0.32.0 (#7):
  - `get_view_definition` and `get_views` return the view's query. They
    returned the whole `CREATE VIEW S.V AS SELECT ...` statement Db2
    stores, which Tabularis's view editor showed as the query and sent back
    to be wrapped in a second `CREATE OR REPLACE VIEW`.
  - A definition longer than 4,096 bytes is returned whole.
  - `get_view_definition` reports a view that does not exist instead of
    returning an empty definition.
- Index management, verified against Db2 12.1 and Tabularis 0.32.0 (#8):
  - `drop_index` looks up the schema the index lives in. It assumed the
    table's schema and wrote it unquoted, so an index created in another
    schema, or in a schema with a delimited name, could not be dropped.
  - An index that is not on the table is refused with `Index "X" was not
    found on table "S"."T"` instead of dropping an index of the same name
    that belongs to another table.
  - `get_indexes` reports the expression of an expression-based index
    (`UPPER(LAST_NAME)`) instead of the column name Db2 generates for it
    (`K00`), marked `is_expression`, and leaves out `INCLUDE` columns,
    which are not part of the key.
- Foreign key management, verified against Db2 12.1 and Tabularis 0.32.0
  (#9):
  - `get_foreign_keys`, `get_all_foreign_keys_batch` and
    `get_schema_snapshot` spell out the referential actions (`CASCADE`,
    `SET NULL`, `RESTRICT`, `NO ACTION`). They returned the catalog's
    one-letter codes (`C`, `N`, `R`, `A`), which Tabularis showed as they
    were.
  - Each entry carries `seq_in_fk`, the column's position in its
    constraint, so a composite foreign key is shown and regenerated with
    its columns in key order.
  - `drop_foreign_key` reports `Foreign key "X" was not found on table
    "S"."T"` for a constraint that is not a foreign key of that table. Db2's
    own message only said that the bare name is undefined.
- Routine support, verified against Db2 12.1 and Tabularis 0.32.0 (#10):
  - `get_routines` reports `FUNCTION` and `PROCEDURE`. It returned the
    catalog's `F` and `P`, so Tabularis filed every routine under neither
    Functions nor Procedures.
  - Routines of one name are told apart: each entry carries
    `identity_args`, its argument types (`DECIMAL(10,2), CHAR(3)`), which
    Tabularis shows next to the name and sends back. `get_routine_parameters`
    and `get_routine_definition` answer for that routine; they used to mix
    the parameters of every overload and return the first definition.
    `routine_type` keeps a function and a procedure of the same name apart.
  - Parameters have the right direction: `INOUT` parameters were reported as
    `IN`, and the `B` row type was never mapped. Types carry their length,
    precision and scale (`DECIMAL(15,2)`, not `DECIMAL`).
  - A function's return value is listed first, without a name, as Tabularis
    expects; a table function lists the columns it returns after its
    arguments. Neither was told apart from an argument before.
  - Routines of a module are no longer listed: they are not called by
    schema and name.
  - `get_routine_definition` of a routine Db2 keeps no source for (external
    and built-in routines) says why, instead of opening an empty editor; an
    unknown routine is an error, not an empty definition.
- `explain_query`, checked against Db2 12.1 and Tabularis 0.32.0:
  - Each request now reads its own plan, under a `QUERYTAG` of its own. It
    used to read whichever plan anyone on the database had explained last.
    Its rows are deleted once read, so the explain tables no longer grow with
    every click.
  - Analyze used to set `has_analyze_data` without running anything. It now
    runs the statement and reports the rows it returned or changed and its
    elapsed time.
  - A statement that cannot be explained fails with Db2's error for the
    statement as written, not one quoting the `EXPLAIN` around it. A
    statement with no access plan, such as DDL, says so.
- `SQL0668N` with reason code 7 (table in "reorg pending" state) now comes
  with the `REORG TABLE` statement that fixes it.

### Added

- Visual EXPLAIN. The manifest declares the `explain` capability, and
  `explain_query` answers in Tabularis's raw form (`engine: db2`,
  `format: db2-explain-tables-json`). The answer carries the rows Db2 wrote
  into its explain tables, and a TypeScript parser shipped with the plugin
  (`explain/`, declared under `explain_parsers`) turns them into the plan
  tree, as the SQL Server plugin does with SHOWPLAN XML. Compared with the
  tree the plugin used to build:
  - tables and indexes are the `relation` of the operator that reads them,
    no longer operators of their own named `Unknown` with id `-1`. An index
    scan names its index and its table.
  - predicates are placed where Db2 applies them: index ranges in
    `index_condition`, filters in `filter`, the join of a hash join in
    `hash_condition`.
  - joins report their type, and a join's outer input comes first.
  - operator arguments (sort keys, prefetch, lock intent…) and Db2's compile
    time are kept.
- `create_view`, `alter_view` and `drop_view`. The manifest declares
  `views`, so Tabularis offers "Create New View", "Edit View" and "Drop
  View"; all three answered "Method not implemented". `alter_view` keeps
  the explicit column list of a view that has one.
- `get_table_ddl`: the `CREATE TABLE` of an existing table rebuilt from the
  catalog (full types, defaults, identity and generated columns, primary
  key and unique constraints, indexes, comments). "Export table to SQL" and
  "Dump Database" use it; the dump failed without it.
- Column descriptions in the table designer: the manifest declares
  `column_comments`, and the generated DDL carries `COMMENT ON COLUMN`.
- `fetch_blob_as_data_url` and `save_blob_to_file`: the whole value of a
  binary column, for the preview in the row editor and for export (a query
  result only carries the first 10,240 bytes).
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

- Tabularis 0.30.0 or later is required (it was 0.25.0): the first release
  that serves a plugin's EXPLAIN parser to Tabularis Web.
- Release archives include `explain/dist/index.iife.js`; a manual install
  copies the `explain/` folder along with the binary and `.tabularium`.
- Each request now uses a single Db2 connection for all of its statements
  (`get_schema_snapshot` opened three).
- `connect_timeout_secs` from the connection settings is honoured as the
  login timeout.

### Dependencies

- `odbc-api` 24.1.1 → 29.3.0 (#19). The raw ODBC paths for the IBM
  clidriver's 32-bit lengths are unchanged and pass the live Db2 suite.
- `serde` 1.0.228 → 1.0.229 (#20).
- `serde_json` 1.0.149 → 1.0.154 (#21).
- The Visual EXPLAIN parser stays on TypeScript 5.9: TypeScript 7 drops the
  compiler API its declaration build needs (#33).

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
