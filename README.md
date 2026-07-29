<div align="center">
  <img src="https://raw.githubusercontent.com/debba/tabularis/main/public/logo-sm.png" width="120" height="120" />
</div>

# tabularis-db2-plugin
<p align="center">

![](https://img.shields.io/badge/status-WIP-red?style=flat)
[![Discord](https://img.shields.io/discord/1502944695808950282?color=5865F2&logo=discord&logoColor=white)](https://discord.com/invite/K2hmhfHRSt)

</p>

> **Work in progress** — this plugin is under active development. See the [verification checklist](#verification-checklist) below for the current status.

An [IBM Db2](https://www.ibm.com/products/db2) plugin for [Tabularis](https://github.com/debba/tabularis), the lightweight database management tool.

This plugin enables Tabularis to connect to Db2 through **ODBC**, browse schemas and objects, run SQL queries, perform inline CRUD operations, and generate DDL through the Tabularis JSON-RPC plugin interface.

**Discord** - [Join our discord server](https://discord.com/invite/K2hmhfHRSt) and chat with the maintainers.

## Table of Contents

- [Verification Checklist](#verification-checklist)
- [Features](#features)
- [Supported Db2 Data Types](#supported-db2-data-types)
- [Requirements](#requirements)
- [Installation](#installation)
  - [Automatic (via Tabularis)](#automatic-via-tabularis)
  - [Manual Installation](#manual-installation)
- [How It Works](#how-it-works)
- [Supported Operations](#supported-operations)
- [Building from Source](#building-from-source)
- [Development](#development)
  - [Docker-based Db2 Environment](#docker-based-db2-environment)
- [Known Limitations](#known-limitations)
- [License](#license)

## Verification Checklist

| Feature | Status | Issue |
|---|---|---|
| Read table data | Verified | — |
| Schema & object browsing | Not yet verified | [#1](https://github.com/debba/tabularis-db2-plugin/issues/1) |
| Query execution | Not yet verified | [#2](https://github.com/debba/tabularis-db2-plugin/issues/2) |
| Insert record | Not yet verified | [#3](https://github.com/debba/tabularis-db2-plugin/issues/3) |
| Update record | Not yet verified | [#4](https://github.com/debba/tabularis-db2-plugin/issues/4) |
| Delete record | Not yet verified | [#5](https://github.com/debba/tabularis-db2-plugin/issues/5) |
| DDL generation | Not yet verified | [#6](https://github.com/debba/tabularis-db2-plugin/issues/6) |
| View support | Not yet verified | [#7](https://github.com/debba/tabularis-db2-plugin/issues/7) |
| Index management | Not yet verified | [#8](https://github.com/debba/tabularis-db2-plugin/issues/8) |
| Foreign key management | Not yet verified | [#9](https://github.com/debba/tabularis-db2-plugin/issues/9) |
| Routine support | Not yet verified | [#10](https://github.com/debba/tabularis-db2-plugin/issues/10) |
| Explain query | Verified | [#11](https://github.com/debba/tabularis-db2-plugin/issues/11) |

## Features

- **Connection via ODBC** — Connect to Db2 using host, port, database, credentials, and optional plugin settings.
- **Schema & Object Browsing** — List schemas, tables, columns, views, indexes, foreign keys, and routines.
- **Query Execution** — Run arbitrary SQL queries with pagination support.
- **Inline Editing** — Insert, update, and delete rows directly from the Tabularis grid.
- **DDL Generation** — Generate `CREATE TABLE`, `ALTER TABLE`, `CREATE INDEX`, and `ADD FOREIGN KEY` SQL.
- **Modern Plugin Manifest** — Uses current Tabularis plugin manifest fields including `settings`, modern capabilities, color/icon metadata, and connection string support.
- **Modular Rust Codebase** — RPC, client creation, metadata handlers, CRUD, DDL, and utilities are split into dedicated modules with unit tests.

## Supported Db2 Data Types

| Category | Types |
|---|---|
| **Numeric** | SMALLINT, INTEGER, BIGINT, DECIMAL, DECFLOAT, REAL, DOUBLE |
| **String** | CHAR, VARCHAR, CLOB, GRAPHIC, VARGRAPHIC, DBCLOB |
| **Date/Time** | DATE, TIME, TIMESTAMP |
| **Binary** | BLOB, BINARY, VARBINARY |
| **Other** | XML, JSON |

## Requirements

This plugin uses the [`odbc-api`](https://crates.io/crates/odbc-api) crate and requires a working Db2 ODBC driver on the host system.

Typical requirement:

- IBM Db2 ODBC driver installed
- ODBC driver manager available on the OS
- network connectivity to the Db2 server

You can configure the driver name from Tabularis through the plugin settings.

## Installation

### Automatic (via Tabularis)

Once the plugin is packaged and published in a registry-compatible format, it can be installed through the Tabularis plugin UI.

### Manual Installation

1. Build the plugin in release mode:

```bash
cargo build --release
```

2. Copy `tabularis-db2-plugin` (or `tabularis-db2-plugin.exe` on Windows) and `.tabularium` into the Tabularis plugins directory:

| OS | Plugins Directory |
|---|---|
| **Linux** | `~/.local/share/tabularis/plugins/db2/` |
| **macOS** | `~/Library/Application Support/com.debba.tabularis/plugins/db2/` |
| **Windows** | `%APPDATA%\com.debba.tabularis\plugins\db2\` |

3. Restart Tabularis.

### Local Sync Helper

A convenience script is provided to build and copy the plugin into the local Tabularis plugins directory:

```bash
./sync.sh
```

## How It Works

The plugin is a standalone Rust binary that communicates with Tabularis through **JSON-RPC 2.0 over stdio**:

1. Tabularis spawns the plugin process.
2. Requests are sent as newline-delimited JSON-RPC messages to the plugin `stdin`.
3. The plugin opens Db2 ODBC connections on demand and writes JSON-RPC responses to `stdout`.

The code is intentionally split by responsibility:

- `src/main.rs` — request loop and dispatch
- `src/client.rs` — ODBC connection string construction and query execution
- `src/handlers/metadata.rs` — schemas, tables, columns, views, indexes, FKs, routines
- `src/handlers/query.rs` — connection test, query execution, explain plan
- `src/handlers/crud.rs` — insert, update, delete
- `src/handlers/ddl.rs` — SQL generation helpers
- `src/utils/` — pure helpers with unit tests

## Supported Operations

| Method | Description |
|---|---|
| `initialize` | Loads plugin settings sent by Tabularis |
| `test_connection` / `ping` | Verifies Db2 connectivity |
| `get_databases` | Returns the current server/database identity |
| `get_schemas` | Lists Db2 schemas |
| `get_tables` | Lists base tables in the selected schema |
| `get_columns` | Returns column metadata for a table or view |
| `get_foreign_keys` | Lists foreign keys for a table |
| `get_indexes` | Lists indexes for a table |
| `get_views` | Lists views in the selected schema |
| `get_view_definition` | Returns the stored view definition |
| `get_view_columns` | Returns column metadata for a view |
| `get_routines` | Lists routines in the selected schema |
| `get_routine_parameters` | Returns routine parameters |
| `get_routine_definition` | Returns the stored routine body when available |
| `execute_query` | Runs SQL with pagination for `SELECT` / `WITH` queries |
| `explain_query` | Returns the DB2 execution plan tree by reading explain tables |
| `insert_record` | Inserts a row |
| `update_record` | Updates a row by primary key column |
| `delete_record` | Deletes a row by primary key column |
| `get_schema_snapshot` | Returns all tables with columns and foreign keys |
| `get_all_columns_batch` | Returns all column metadata for a schema |
| `get_all_foreign_keys_batch` | Returns all FK metadata for a schema |
| `get_create_table_sql` | Generates `CREATE TABLE` SQL |
| `get_add_column_sql` | Generates `ALTER TABLE ... ADD COLUMN` SQL |
| `get_alter_column_sql` | Generates `ALTER TABLE ... ALTER/RENAME COLUMN` SQL |
| `get_create_index_sql` | Generates `CREATE INDEX` SQL |
| `get_create_foreign_key_sql` | Generates FK DDL |
| `drop_index` | Executes `DROP INDEX` |
| `drop_foreign_key` | Executes `ALTER TABLE ... DROP FOREIGN KEY` |

## Building from Source

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (edition 2021)
- Db2 ODBC driver installed locally

### Build

```bash
cargo build --release
```

The binary will be available at:

```bash
target/release/tabularis-db2-plugin
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
echo -e "[Db2]\nDescription = IBM Db2 ODBC Driver\nDriver = /opt/ibm/clidriver/lib/libdb2o.so" \
  | sudo tee /etc/odbcinst.ini
```

#### Start, seed, and manage the test database

A helper script handles the full lifecycle:

```bash
# Start the container and seed TEST_SCHEMA with fixtures
./scripts/setup-test-db.sh

# Check container health and seed status
./scripts/setup-test-db.sh status

# Stop and remove the container (including volumes)
./scripts/setup-test-db.sh teardown
```

The first start can take a couple of minutes while Db2 initialises. The script waits up to 300 seconds and streams progress.

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
DB2_TEST=1 cargo test --test integration -- --test-threads=1
```

> Integration tests are gated behind the `DB2_TEST` environment variable so that `cargo test` alone only runs unit tests (no Db2 connection required).

### Run Unit Tests

```bash
cargo test
```

### Smoke Test the Plugin Process

```bash
cargo run --bin test_plugin
```

### Manual JSON-RPC example

```bash
echo '{"jsonrpc":"2.0","method":"initialize","params":{"settings":{}},"id":1}' \
  | ./target/debug/tabularis-db2-plugin
```

### Tech Stack

- **Language:** Rust (edition 2021)
- **Database driver:** [`odbc-api`](https://crates.io/crates/odbc-api)
- **Serialization:** serde + serde_json
- **Protocol:** JSON-RPC 2.0 over stdio

## Known Limitations

- The plugin requires a working Db2 ODBC driver on the host system.
- `explain_query` reads the DB2 explain tables (`SYSTOOLS.EXPLAIN_OPERATOR`, `EXPLAIN_STREAM`, `EXPLAIN_STATEMENT`) to build a plan tree; these tables must exist (see `CALL SYSPROC.SYSINSTALLOBJECTS`). Some cost/row metrics depend on the DB2 edition and optimizer statistics being up to date.
- Catalog queries were designed using Db2 catalog conventions and DBeaver’s Db2 extension as a reference, but they should still be validated against the specific Db2 edition/version you target.
- `get_databases` is intentionally conservative and currently returns the active server/database identity instead of enumerating every possible database on an instance.

## License

Apache License 2.0
