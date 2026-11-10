<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/TabularisDB/tabularis/main/.github/assets/banner-dark.png" />
    <img src="https://raw.githubusercontent.com/TabularisDB/tabularis/main/.github/assets/banner-light.png" alt="tabularis — the database client your AI agent can actually use" width="600" />
  </picture>

  <br/>

  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/TabularisDB/tabularis-db2-plugin/master/assets/db2-banner-dark.png" />
    <img src="https://raw.githubusercontent.com/TabularisDB/tabularis-db2-plugin/master/assets/db2-banner-light.png" alt="Native Db2 support" width="600" />
  </picture>
</div>

# tabularis-db2-plugin

<p align="center">

![Release](https://img.shields.io/github/v/release/TabularisDB/tabularis-db2-plugin?include_prereleases&style=flat)
![Downloads](https://img.shields.io/github/downloads/TabularisDB/tabularis-db2-plugin/total.svg?style=flat)
![CI](https://github.com/TabularisDB/tabularis-db2-plugin/workflows/CI/badge.svg)
[![Discord](https://img.shields.io/discord/1502944695808950282?color=5865F2&logo=discord&logoColor=white)](https://discord.com/invite/K2hmhfHRSt)

</p>

An [IBM Db2](https://www.ibm.com/products/db2) plugin for [Tabularis](https://github.com/TabularisDB/tabularis), the open-source database client.

This plugin enables Tabularis to connect to Db2 through **ODBC**, browse schemas and objects, run SQL queries, perform inline CRUD operations, and generate DDL through the Tabularis JSON-RPC plugin interface.

> **Requires Tabularis v0.30.0 or later.** That is the first release that
> serves a plugin's Visual EXPLAIN parser to Tabularis Web as well as the
> desktop app; the plugin will not load on earlier versions of
> [Tabularis](https://github.com/TabularisDB/tabularis).

**Discord** - [Join our discord server](https://discord.com/invite/K2hmhfHRSt) and chat with the maintainers.

## Table of Contents

- [Verification Checklist](#verification-checklist)
- [Features](#features)
- [Supported Db2 Data Types](#supported-db2-data-types)
- [Requirements](#requirements)
- [Installation](#installation)
  - [From the Tabularium registry](#from-the-tabularium-registry)
  - [Manual Installation (alternative)](#manual-installation-alternative)
- [How It Works](#how-it-works)
- [Supported Operations](#supported-operations)
- [Building from Source](#building-from-source)
- [Development](#development)
  - [Docker-based Db2 Environment](#docker-based-db2-environment)
  - [Contributing: PR Titles & Versioning](#contributing-pr-titles--versioning)
- [Known Limitations](#known-limitations)
- [Changelog](#changelog)
- [Maintainers](#maintainers)
- [License](#license)

## Verification Checklist

Every feature below was verified against Db2 12.1 Community Edition and
Tabularis 0.32.0, through the live integration suite and in the Tabularis UI;
the linked issues carry the results.

| Feature | Status | Issue |
|---|---|---|
| Read table data | Verified | — |
| Schema & object browsing | Verified | [#1](https://github.com/TabularisDB/tabularis-db2-plugin/issues/1) |
| Query execution | Verified | [#2](https://github.com/TabularisDB/tabularis-db2-plugin/issues/2) |
| Insert record | Verified | [#3](https://github.com/TabularisDB/tabularis-db2-plugin/issues/3) |
| Update record | Verified | [#4](https://github.com/TabularisDB/tabularis-db2-plugin/issues/4) |
| Delete record | Verified | [#5](https://github.com/TabularisDB/tabularis-db2-plugin/issues/5) |
| DDL generation | Verified | [#6](https://github.com/TabularisDB/tabularis-db2-plugin/issues/6) |
| View support | Verified | [#7](https://github.com/TabularisDB/tabularis-db2-plugin/issues/7) |
| Index management | Verified | [#8](https://github.com/TabularisDB/tabularis-db2-plugin/issues/8) |
| Foreign key management | Verified | [#9](https://github.com/TabularisDB/tabularis-db2-plugin/issues/9) |
| Routine support | Verified | [#10](https://github.com/TabularisDB/tabularis-db2-plugin/issues/10) |
| Explain query | Verified | [#11](https://github.com/TabularisDB/tabularis-db2-plugin/issues/11) |

## Features

- **Connection via ODBC** — Connect to Db2 using host, port, database, credentials, and optional plugin settings.
- **Schema & Object Browsing** — List schemas, tables, columns, views, indexes, foreign keys, and routines.
- **Query Execution** — Run any SQL statement, with pagination that leaves your `ORDER BY`, `FETCH FIRST` and isolation clauses alone, typed values (integers, floats, booleans, exact decimals, BLOB previews) and Db2's own error messages. Multi-statement scripts run on one connection, and `BEGIN ... END` blocks stay one statement.
- **Inline Editing** — Insert, update, and delete rows directly from the Tabularis grid, including tables without a primary key: the row is matched on all its columns and exactly one row is changed.
- **DDL Generation** — Generate `CREATE TABLE`, `ALTER TABLE`, `CREATE INDEX`, and `ADD FOREIGN KEY` SQL that Db2 runs as is: quoted identifiers, identity columns, column comments, and the `REORG` a column change needs. Existing tables can be exported and dumped with their real definition.
- **Modern Plugin Manifest** — Uses current Tabularis plugin manifest fields including `settings`, modern capabilities, color/icon metadata, and connection string support.
- **Modular Rust Codebase** — RPC, client creation, metadata handlers, CRUD, DDL, and utilities are split into dedicated modules with unit tests.

## Supported Db2 Data Types

| Category | Types |
|---|---|
| **Numeric** | SMALLINT, INTEGER, BIGINT, DECIMAL, DECFLOAT, REAL, DOUBLE |
| **String** | CHAR, VARCHAR, CLOB, GRAPHIC, VARGRAPHIC, DBCLOB |
| **Date/Time** | DATE, TIME, TIMESTAMP |
| **Binary** | BLOB, BINARY, VARBINARY |
| **Other** | BOOLEAN, XML |

## Requirements

This plugin uses the [`odbc-api`](https://crates.io/crates/odbc-api) crate and requires a working Db2 ODBC driver on the host system.

Typical requirement:

- IBM Db2 ODBC driver installed
- ODBC driver manager available on the OS
- network connectivity to the Db2 server

You can configure the driver name from Tabularis through the plugin settings.

On Linux and macOS the driver manager is [unixODBC](https://www.unixodbc.org/)
(`apt install unixodbc`, `brew install unixodbc`); Windows ships its own. The
IBM driver itself needs `libxml2.so.2`: distributions that only ship the newer
`libxml2.so.16` (for example Ubuntu 25.10 and later) need a compatibility
package providing it, otherwise the driver manager reports
`Can't open lib '.../libdb2.so' : file not found`.

## Installation

### From the Tabularium registry

This plugin is published on the [Tabularium registry](https://registry.tabularis.dev).
Install it from Tabularis's in-app plugin browser: **Settings → Plugins**,
search for **Db2**, and install.

### Manual Installation (alternative)

1. Download the latest release for your platform from the
   [Releases page](https://github.com/TabularisDB/tabularis-db2-plugin/releases).
2. Extract the archive.
3. Copy everything the archive contains (`tabularis-db2-plugin` or
   `tabularis-db2-plugin.exe`, `.tabularium` and the `explain/` folder with
   the Visual EXPLAIN parser) into the Tabularis plugins directory:

   | OS | Plugins Directory |
   | --- | --- |
   | **Linux** | `~/.local/share/tabularis/plugins/drivers/db2/` |
   | **macOS** | `~/Library/Application Support/tabularis/plugins/drivers/db2/` |
   | **Windows** | `%APPDATA%\tabularis\plugins\drivers\db2\` |

4. Restart Tabularis.

## How It Works

The plugin is a standalone Rust binary that communicates with Tabularis through **JSON-RPC 2.0 over stdio**:

1. Tabularis spawns the plugin process.
2. Requests are sent as newline-delimited JSON-RPC messages to the plugin `stdin`.
3. The plugin opens Db2 ODBC connections on demand and writes JSON-RPC responses to `stdout`.

The code is intentionally split by responsibility:

- `src/main.rs` — request loop and dispatch
- `src/client.rs` — ODBC connection string construction, one connection per request, text fetching
- `src/handlers/metadata.rs` — schemas, tables, columns, views, indexes, FKs, routines
- `src/handlers/query.rs` — connection test, query execution
- `src/handlers/explain.rs` — `EXPLAIN PLAN`, read back from the explain tables
- `src/handlers/crud.rs` — insert, update, delete
- `src/handlers/ddl.rs` — SQL generation helpers
- `src/utils/` — pure helpers with unit tests
- `explain/` — the Visual EXPLAIN parser (TypeScript): turns the explain-table
  rows `explain_query` returns into the plan tree Tabularis draws. It is built
  into `explain/dist/index.iife.js`, declared under `explain_parsers` in
  `.tabularium` and shipped in every release archive, the same way as the SQL
  Server plugin's SHOWPLAN parser.

## Supported Operations

| Method | Description |
|---|---|
| `initialize` | Loads plugin settings sent by Tabularis |
| `test_connection` / `ping` | Verifies Db2 connectivity |
| `get_databases` | Returns the current server/database identity |
| `get_schemas` | Lists the user schemas, empty ones included (Db2's own `SYS*`, `NULLID` and `SQLJ` are left out) |
| `get_tables` | Lists the tables of the selected schema: base, typed, materialized query and created temporary tables, and nicknames |
| `get_columns` | Returns column metadata for a table or view: type with length/precision/scale, nullability, key, identity, generated flag, default and comment |
| `get_foreign_keys` | Lists the foreign keys of a table, one entry per column: referenced table and column, `ON DELETE` / `ON UPDATE` rule, and position within a composite key |
| `get_indexes` | Lists the indexes of a table, one entry per key column: uniqueness, primary key, position, and the expression of an expression-based index |
| `get_views` | Lists views in the selected schema |
| `get_view_definition` | Returns the view's query: the stored `CREATE VIEW` text without its header |
| `get_view_columns` | Returns column metadata for a view |
| `create_view` / `alter_view` / `drop_view` | Create a view from a query, replace its query (`CREATE OR REPLACE VIEW`, keeping an explicit column list) and drop it |
| `get_routines` | Lists functions and procedures, with the argument types that tell overloads apart |
| `get_routine_parameters` | Returns a routine's parameters with their direction, its return value or returned columns |
| `get_routine_definition` | Returns the `CREATE` statement of an SQL routine |
| `execute_query` | Runs one statement. A result set (`SELECT`, `WITH`, `VALUES`, `CALL`) is paged on the cursor and returned with typed values; anything else returns the number of rows it changed |
| `execute_query_batch` | Runs the statements of a script in order on one connection, reporting each one's result or error |
| `explain_query` | Returns Db2's access plan in Tabularis's raw form (`engine: db2`, `format: db2-explain-tables-json`): the rows of the explain tables, which the plugin's parser draws with tables and indexes, row estimates, costs, predicates and join types. With `analyze`, the statement also runs and its real row count and time are reported |
| `insert_record` | Inserts a row, rendering each value for its column type (numbers validated, text and binary data bound as parameters, `NULL`, column defaults, BLOB uploads) |
| `fetch_blob_as_data_url` / `save_blob_to_file` | Read the whole value of a binary column for the preview and for export |
| `update_record` | Sets one column of one row, found by its primary key (single or composite) or, for a table without one, by the values of all its columns |
| `delete_record` | Deletes one row, found the same way as for `update_record` |
| `get_schema_snapshot` | Returns all tables with columns and foreign keys |
| `get_all_columns_batch` | Returns all column metadata for a schema |
| `get_all_foreign_keys_batch` | Returns all FK metadata for a schema |
| `get_create_table_sql` | Generates `CREATE TABLE` (identity columns, defaults, primary key) and `COMMENT ON COLUMN` statements |
| `get_add_column_sql` | Generates `ALTER TABLE ... ADD COLUMN`; a `NOT NULL` column without a default gets `WITH DEFAULT`, which Db2 requires on a table with rows |
| `get_alter_column_sql` | Generates one statement per changed attribute (rename, type, nullability, default, identity, comment), followed by `REORG TABLE` when the change requires it |
| `get_create_index_sql` | Generates `CREATE [UNIQUE] INDEX` in the table's schema |
| `get_create_foreign_key_sql` | Generates `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY`; referential actions Db2 does not have (`ON UPDATE CASCADE`) are refused |
| `get_table_ddl` | Rebuilds the `CREATE TABLE` of an existing table from the catalog, with its indexes and comments, for table export and database dumps |
| `drop_index` | Drops an index of the table, in whichever schema the index lives |
| `drop_foreign_key` | Drops a foreign key of the table (`ALTER TABLE ... DROP FOREIGN KEY`) |

## Building from Source

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2021)
- [`just`](https://github.com/casey/just) (optional, wraps the common cargo invocations)
- [Node.js](https://nodejs.org/) 22.18 or later and [pnpm](https://pnpm.io/)
  (the version pinned in `explain/package.json`), for the Visual EXPLAIN parser
- The ODBC driver manager development files (`unixodbc-dev` on Debian/Ubuntu,
  `brew install unixodbc` on macOS; nothing to install on Windows)
- A Db2 ODBC driver and a running Db2 instance (for integration tests)

### Build

```bash
just build      # debug build, plus the Visual EXPLAIN parser bundle
just release    # release build (what the GitHub Actions workflow ships)
just build-explain   # the parser bundle alone (explain/dist/index.iife.js)
```

Or directly with cargo:

```bash
cargo build --release
```

The binary will be located at `target/release/tabularis-db2-plugin`.

### Install Locally

```bash
just dev-install   # build + copy binary, manifest and parser bundle into the Tabularis plugins dir
just uninstall     # remove the installed plugin
```

## Development

### Docker-based Db2 Environment

A `docker-compose.yml` is provided to spin up a local IBM Db2 Community Edition instance for development and integration testing.

#### Prerequisites

- Docker (or Podman with the `docker` CLI alias)
- IBM Data Server Driver for ODBC and CLI (`clidriver`) installed on the host and registered in `/etc/odbcinst.ini`

**Installing the IBM clidriver on Linux:**

```bash
curl -fSL https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/linuxx64_odbc_cli.tar.gz \
  -o /tmp/clidriver.tar.gz
sudo mkdir -p /opt/ibm
sudo tar xf /tmp/clidriver.tar.gz -C /opt/ibm
echo -e "[Db2]\nDescription = IBM Db2 ODBC Driver\nDriver = /opt/ibm/clidriver/lib/libdb2.so" \
  | sudo tee /etc/odbcinst.ini
```

#### Start, seed, and manage the test database

A helper script handles the full lifecycle (the `just` recipes wrap it):

```bash
# Start the container and seed TEST_SCHEMA with fixtures
just demo-db            # ./scripts/setup-test-db.sh

# Drop the fixture schemas and load the fixtures again (keeps the container)
just demo-db-reseed     # ./scripts/setup-test-db.sh reseed

# Check container health and seed status
just demo-db-status     # ./scripts/setup-test-db.sh status

# Stop and remove the container (including volumes)
just demo-db-stop       # ./scripts/setup-test-db.sh teardown
```

The first start can take several minutes while Db2 creates the database. The script waits up to 600 seconds (override with `DB2_MAX_WAIT`) and streams progress.

#### Connection details

| Parameter | Value |
|---|---|
| **Host** | `localhost` |
| **Port** | `50000` |
| **Database** | `TESTDB` |
| **User** | `db2inst1` |
| **Password** | `db2test123` |
| **Schema** | `TEST_SCHEMA` |

#### Run integration tests

Once the container is healthy:

```bash
just integration   # DB2_TEST=1 cargo test --test integration -- --test-threads=1
```

The tests look for an ODBC driver registered as `Db2`; set `DB2_ODBC_DRIVER`
when your `odbcinst.ini` entry has another name. `DB2_HOST`, `DB2_PORT`,
`DB2_DATABASE`, `DB2_USER` and `DB2_PASSWORD` override the container defaults.
The same suite runs in CI as the "Live Db2 integration" job.

> Integration tests are gated behind the `DB2_TEST` environment variable so that `cargo test` alone only runs unit tests (no Db2 connection required).

### Running Tests

```bash
just test    # cargo test --bins — unit tests for SQL builders, quoting, pagination
just lint    # clippy -D warnings
just fmt     # cargo fmt --all
just validate-manifest   # .tabularium against the live registry schema
just test-explain        # typecheck and test the Visual EXPLAIN parser
```

The parser's tests (`explain/tests/`) run on explain payloads captured from
Db2 12.1 and compare the plans with committed goldens. Regenerate the goldens
with `UPDATE_GOLDENS=1 pnpm --dir explain test` after an intended change, and
review the diff.

### Smoke Test the Plugin Process

```bash
just repl    # cargo run --bin test_plugin
```

### Manual JSON-RPC example

```bash
echo '{"jsonrpc":"2.0","method":"initialize","params":{"settings":{}},"id":1}' \
  | ./target/debug/tabularis-db2-plugin
```

### Contributing: PR Titles & Versioning

PR titles must follow [Conventional Commits](https://www.conventionalcommits.org/)
(`type: subject`, `type(scope): subject`, or `type!: subject` for a breaking
change) — enforced by CI on every PR. Add a `BREAKING CHANGE:` footer to the
PR description for breaking changes that don't fit cleanly into the title.

Every PR also needs exactly one `prerelease:alpha` / `prerelease:beta` /
`prerelease:rc` / `prerelease:stable` label, so CI knows which release
channel to target when suggesting the next version. There's no default —
CI fails with a clear error if the label is missing, rather than guessing.

| PR title type | Version impact |
| --- | --- |
| `feat` | minor |
| `fix`, `refactor`, `perf` | patch |
| `docs`, `style`, `chore`, `test`, `ci`, `build` | none — no release suggested |
| any type with `!` or a `BREAKING CHANGE:` footer | major |

CI posts a comment on the PR suggesting the next tag/version based on the
title's type and the `prerelease:*` label — informational only, nothing is
tagged or released automatically. The suggestion updates (and marks the
previous suggestion as outdated) only when the underlying classification
actually changes, not on every edit to the title text.

To release: bump `version` in `.tabularium` and `Cargo.toml` (CI checks they
agree), add a `## [x.y.z] - YYYY-MM-DD` section to [CHANGELOG.md](./CHANGELOG.md),
merge, then push the `vx.y.z` tag. The release workflow refuses a tag that
does not match the manifest, builds every platform, smoke-tests each binary
and publishes the release with that changelog section as its notes.

### Tech Stack

- **Language:** Rust (edition 2021)
- **Database driver:** [`odbc-api`](https://crates.io/crates/odbc-api)
- **Serialization:** serde + serde_json
- **Visual EXPLAIN parser:** TypeScript, bundled with tsup, tested with vitest
- **Protocol:** JSON-RPC 2.0 over stdio

## Known Limitations

- The plugin requires a working Db2 ODBC driver on the host system.
- `explain_query` writes the plan into the Db2 explain tables (the session user's own, else `SYSTOOLS`) and deletes it once read. When neither schema has them, they are created in `SYSTOOLS` with `SYSPROC.SYSINSTALLOBJECTS`, which needs the authority to create tables there. Costs and row estimates are the optimizer's, and depend on up-to-date statistics (`RUNSTATS`).
- Analyze runs the statement (a data change is committed) and reports its real row count and elapsed time for the statement as a whole. Per-operator actuals would need an activity event monitor, which a client cannot assume it may create.
- Catalog queries were designed using Db2 catalog conventions and DBeaver’s Db2 extension as a reference, but they should still be validated against the specific Db2 edition/version you target.
- Every statement is committed on its own: connections run in autocommit mode and are not kept between requests, so an explicit transaction cannot span several statements run from the editor. Wrap the statements in a `BEGIN ... END` compound statement to run them atomically.
- A `BLOB` in a query result carries its first 10,240 bytes and its full size; `BINARY`, `VARBINARY` and `FOR BIT DATA` values are shown as hexadecimal text.
- Paging skips rows on the cursor, so a very deep page of a very large result takes longer than the first ones. Filter or order the result to get to the rows you need.
- Changing a column's type or nullability, or dropping a column, leaves a Db2 table in "reorg pending" state. The statements generated by the column editor end with the `REORG TABLE` that clears it; after an `ALTER TABLE` of your own (or the "Delete Column" action), run the `CALL SYSPROC.ADMIN_CMD('REORG TABLE ...')` the error message suggests.
- Db2 only supports `NO ACTION` and `RESTRICT` for `ON UPDATE`, and has no `SET DEFAULT` referential action; the foreign key editor reports that instead of generating SQL that cannot run.
- A dump or export keeps identity columns as they are defined. Restoring rows into a `GENERATED ALWAYS` identity column fails (`SQL0798N`), as Db2 does not accept explicit values there; foreign keys are not part of the generated `CREATE TABLE`.
- Db2 has no `JSON` column type (JSON is stored in `CLOB`, `VARCHAR` or `BLOB` columns), so it is not offered when creating a column.
- Routines are read-only: they are listed, described and opened, but creating, running or dropping one goes through the SQL editor. External and built-in routines have no stored source, so only SQL routines can be opened.
- `get_databases` is intentionally conservative and currently returns the active server/database identity instead of enumerating every possible database on an instance.

## [Changelog](./CHANGELOG.md)

## Maintainers

- @debba

## License

Apache-2.0.
