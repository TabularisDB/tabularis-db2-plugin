# CLAUDE.md

## Status

This repo is the home of the IBM Db2 driver plugin for Tabularis, published
to the Tabularium registry as `db2`. It requires Tabularis v0.30.0 or later,
the first release that serves a plugin's EXPLAIN parser bundle in Tabularis
Web too (and reads the manifest's stable `id` separately from its display
`name`) — see `.tabularium`'s `min_runtime_version` field.

## Build & Test

The binary links against the ODBC driver manager (unixODBC on Linux/macOS,
`odbc32` on Windows), so `unixodbc-dev` (or `brew install unixodbc`) must be
installed before building.

```bash
cargo build              # Debug build
cargo build --release    # Release build
cargo test --bins        # Unit tests (no Db2 needed)
cargo clippy --all-targets -- -D warnings  # Lint
cargo fmt --all          # Format
```

The Visual EXPLAIN parser in `explain/` is TypeScript (Node 22.18+, pnpm):

```bash
pnpm --dir explain install --frozen-lockfile
pnpm --dir explain typecheck
pnpm --dir explain test  # builds explain/dist/index.iife.js, then vitest
```

`just` wraps the same commands (`just build`, `just test`, `just lint`,
`just fmt`, `just test-explain`, `just dev-install`).

## Live Db2 Tests

`tests/integration.rs` spawns the plugin binary and talks JSON-RPC over
stdio, exactly as Tabularis does. It is gated behind `DB2_TEST=1` so a plain
`cargo test` never needs a database.

```bash
just demo-db       # Db2 Community Edition in Docker, seeded with TEST_SCHEMA
just integration   # DB2_TEST=1 cargo test --test integration -- --test-threads=1
just demo-db-stop
```

The tests need the IBM Data Server Driver for ODBC and CLI (`clidriver`)
registered in `odbcinst.ini` as `Db2`; set `DB2_ODBC_DRIVER` when your entry
has another name. `DB2_HOST`, `DB2_PORT`, `DB2_DATABASE`, `DB2_USER` and
`DB2_PASSWORD` override the container defaults. The same suite runs in CI as
the "Live Db2 integration" job.

Fixtures live in `tests/fixtures/` and are loaded by
`scripts/setup-test-db.sh`. A test that mutates data must leave the fixtures
as it found them, or use the tables reserved for that (`CRUD_TEST`,
`DROP_TEST`): the suite runs single-threaded against one shared database.
`just demo-db-reseed` drops the fixture schemas and loads them again without
recreating the container.

## Architecture Rules

- `src/main.rs` owns the JSON-RPC loop and dispatch. Handlers live in
  `src/handlers/`, organized by domain (`metadata`, `query`, `explain`, `crud`, `ddl`),
  and return `PluginResult<T>` (`Result<T, String>`).
- Db2 access goes through `src/client.rs` (ODBC via `odbc-api`). A request
  opens one `Session` (one connection) and runs all of its statements on it;
  the selected schema is applied with `SET CURRENT SCHEMA '<name>'`, which
  keeps delimited names intact. Result sets are fetched as text with raw
  `SQLFetch`/`SQLGetData` calls: the IBM clidriver writes only 32-bit length
  indicators on 64-bit platforms, which `odbc-api`'s own buffers misread.
  Keep new fetch code on that path.
- `Session::run` is the one way to run user SQL: it lets Db2 say whether
  the statement returned a result set, pages on the cursor (never by
  rewriting the SQL) and reads row counts and column types through raw ODBC
  calls, again because of the 32-bit values the driver writes. Do not call
  `odbc-api`'s `col_data_type` or `row_count`, and do not let a cursor drop:
  `release_cursor` exists because closing a cursor Db2 already closed
  panics.
- Result values are typed by `utils::values::cell_to_json` from the column's
  `ColumnKind`. `DECIMAL`/`DECFLOAT` stay text on purpose (no digit lost).
- Values written to Db2 go through `utils::binding::bind_value`, which
  renders a JSON value for the type of its column: numbers and booleans as
  validated literals, text and binary data as bound parameters (`Param`),
  never as text spliced into the statement. `crud::row_condition` builds
  the `WHERE` of a row from the host's `pk_map` the same way.
- `handlers::ddl` builds the table designer's SQL without a connection.
  Quote every identifier, keep one statement per change, and end with
  `reorg_sql` whenever a statement leaves the table in "reorg pending"
  state (a type or nullability change). Anything Db2 cannot do is an error
  from the builder, never SQL that will fail when run. Each builder has an
  integration test that runs its output on Db2.
- `handlers::explain` tags each `EXPLAIN PLAN` with its own `QUERYTAG`,
  reads only those rows and deletes them. Never read a plan by "latest
  `EXPLAIN_TIME`". It returns the rows as they are, in Tabularis's raw form
  (`engine: db2`, `format: db2-explain-tables-json`): the tree is built by
  the TypeScript parser in `explain/`, declared under `explain_parsers` in
  `.tabularium`. `Db2ExplainPayload` (`src/models.rs`) and
  `explain/src/payload.ts` describe the same JSON: change them together,
  recapture the fixtures in `explain/tests/fixtures/` from Db2 when the
  rows change, and review the regenerated goldens.
- ODBC errors reach the user through `error::odbc_error`, which keeps the
  Db2 message and drops the ODBC call name and driver tags.
- Pure helpers (identifier quoting, literals, pagination math, type
  rendering) live in `src/utils/` with unit tests next to them.
- Never write to `stdout` outside the JSON-RPC responses in `main.rs` — any
  stray write corrupts the protocol stream. Use `eprintln!` for diagnostics.

## Key Patterns

- **Adding a new RPC method**: add it to the dispatch in `main.rs`, implement
  it in the matching `handlers/` module, and list it in the README's
  "Supported Operations" table. Unknown methods must answer `-32601` with a
  message containing `Method not found`: the host's fallbacks for optional
  methods match on it.
- **The host contract is the source of truth**: parameter names and result
  shapes come from Tabularis's `src-tauri/src/plugins/driver.rs` and
  `plugins/PLUGIN_GUIDE.md`. When the two disagree, `driver.rs` is what is
  actually sent on the wire.
- **Capabilities**: declare only capabilities the registry schema knows
  (`npx @tabularium/cli validate`, run by CI) and that the plugin really
  implements. A declared capability shows UI in Tabularis; an RPC behind it
  that fails is a bug.
- **Catalog queries** read `SYSCAT.*` with `WITH UR` and take identifiers as
  SQL string literals: escape single quotes, never interpolate unquoted.
  Identifiers in generated SQL go through `quote_identifier` /
  `quote_qualified_name`.
- **Testing**: prefer the real Db2 container over mocks for anything that
  touches SQL. Extract pure logic into testable functions with unit tests.
- **PR titles and versioning**: PR titles must be Conventional Commits
  (`type: subject`) and every PR needs a `prerelease:alpha|beta|rc|stable`
  label — both enforced by CI. See the README's "Contributing: PR Titles &
  Versioning" section for the full convention and the type→bump mapping.
  CI posts (and keeps up to date) a version-suggestion comment based on
  these — informational only, nothing is tagged or released automatically.
- **Releasing**: bump `version` in `.tabularium`, `Cargo.toml` and
  `explain/package.json` (CI checks they agree), add a `## [x.y.z] - YYYY-MM-DD` section to `CHANGELOG.md`,
  merge, then push the `vx.y.z` tag. The release workflow refuses a tag that
  does not match the manifest and takes the release notes from that
  changelog section.
