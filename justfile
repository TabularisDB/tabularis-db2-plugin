set shell := ["bash", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# Build the plugin binary in debug mode, and the Visual EXPLAIN parser.
build: build-explain
    cargo build

# Build for release (what the GitHub Actions workflow ships).
release: build-explain
    cargo build --release

# Build the Visual EXPLAIN parser bundle (explain/dist/index.iife.js).
build-explain:
    pnpm --dir explain install --frozen-lockfile
    pnpm --dir explain build

# Typecheck and test the Visual EXPLAIN parser.
test-explain:
    pnpm --dir explain install --frozen-lockfile
    pnpm --dir explain typecheck
    pnpm --dir explain test

# tests/integration.rs is excluded: it is a live-database integration test
# requiring a running Db2 instance — see `just integration`, or the
# "Live Db2 integration" CI job.
#
# Run unit tests.
test:
    cargo test --bins

# Needs the Db2 container started by `just demo-db` and the IBM ODBC/CLI
# driver registered as "Db2" (or DB2_ODBC_DRIVER set to your odbcinst.ini
# entry).
#
# Run the live-database integration tests.
integration:
    DB2_TEST=1 cargo test --test integration -- --test-threads=1

# Launch the local smoke test that simulates Tabularis JSON-RPC calls over stdio.
repl:
    cargo run --bin test_plugin

# Run clippy with warnings denied.
lint:
    cargo clippy --all-targets -- -D warnings

# Format the codebase.
fmt:
    cargo fmt --all

# Validate the .tabularium manifest against the live registry schema.
validate-manifest:
    npx --yes @tabularium/cli validate .tabularium --registry https://registry.tabularis.dev --kind driver
    ./scripts/check-capabilities.sh

# Connection: localhost:50000, database TESTDB, user db2inst1, password
# db2test123, schema TEST_SCHEMA.
#
# Start (and seed) a disposable Db2 Community Edition container.
demo-db:
    ./scripts/setup-test-db.sh

# Drop the fixture schemas and load the fixtures again (container kept).
demo-db-reseed:
    ./scripts/setup-test-db.sh reseed

# Show the Db2 container health and seed status.
demo-db-status:
    ./scripts/setup-test-db.sh status

# Stop and remove the Db2 container, including its volume.
demo-db-stop:
    ./scripts/setup-test-db.sh teardown

# ---------------------------------------------------------------------------
# Platform-specific install recipes (plugin-dir conventions per OS).
# ---------------------------------------------------------------------------

[linux]
dev-install: build
    mkdir -p ~/.local/share/tabularis/plugins/drivers/db2
    cp target/debug/tabularis-db2-plugin ~/.local/share/tabularis/plugins/drivers/db2/
    cp .tabularium ~/.local/share/tabularis/plugins/drivers/db2/
    mkdir -p ~/.local/share/tabularis/plugins/drivers/db2/explain/dist
    cp explain/dist/index.iife.js ~/.local/share/tabularis/plugins/drivers/db2/explain/dist/
    @echo "Installed to ~/.local/share/tabularis/plugins/drivers/db2"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[macos]
dev-install: build
    mkdir -p "$HOME/Library/Application Support/tabularis/plugins/drivers/db2"
    cp target/debug/tabularis-db2-plugin "$HOME/Library/Application Support/tabularis/plugins/drivers/db2/"
    cp .tabularium "$HOME/Library/Application Support/tabularis/plugins/drivers/db2/"
    mkdir -p "$HOME/Library/Application Support/tabularis/plugins/drivers/db2/explain/dist"
    cp explain/dist/index.iife.js "$HOME/Library/Application Support/tabularis/plugins/drivers/db2/explain/dist/"
    @echo "Installed to ~/Library/Application Support/tabularis/plugins/drivers/db2"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

# Each recipe line runs in a fresh shell, so this must be one logical command.
# Tabularis resolves its plugin dir via the `directories` crate
# (ProjectDirs "", "", "tabularis"), which on Windows is %APPDATA%\tabularis.
[windows]
dev-install: build
    $dest = Join-Path $env:APPDATA "tabularis\plugins\drivers\db2"; \
    New-Item -ItemType Directory -Force -Path $dest | Out-Null; \
    Copy-Item "target\debug\tabularis-db2-plugin.exe" $dest; \
    Copy-Item ".tabularium" $dest; \
    New-Item -ItemType Directory -Force -Path (Join-Path $dest "explain\dist") | Out-Null; \
    Copy-Item "explain\dist\index.iife.js" (Join-Path $dest "explain\dist"); \
    Write-Host "Installed to $dest"; \
    Write-Host "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[linux]
uninstall:
    rm -rf ~/.local/share/tabularis/plugins/drivers/db2

[macos]
uninstall:
    rm -rf "$HOME/Library/Application Support/tabularis/plugins/drivers/db2"

[windows]
uninstall:
    $dest = Join-Path $env:APPDATA "tabularis\plugins\drivers\db2"; \
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
