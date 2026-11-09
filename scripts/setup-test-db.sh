#!/usr/bin/env bash
# ================================================================
# Setup script for the DB2 integration test environment.
#
# Usage:
#   ./scripts/setup-test-db.sh          # start DB2 + seed data
#   ./scripts/setup-test-db.sh reseed    # drop the fixtures and load them again
#   ./scripts/setup-test-db.sh teardown  # stop + remove container
#   ./scripts/setup-test-db.sh status    # check container health
#
# Prerequisites:
#   1. Docker (or Podman with docker alias)
#   2. IBM Data Server Driver for ODBC and CLI (clidriver)
#      registered in /etc/odbcinst.ini as "Db2" (or set
#      DB2_ODBC_DRIVER to match your odbcinst.ini entry).
#
# Installing the IBM clidriver on Linux:
#   curl -fSL https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/linuxx64_odbc_cli.tar.gz \
#     -o /tmp/clidriver.tar.gz
#   sudo mkdir -p /opt/ibm
#   sudo tar xf /tmp/clidriver.tar.gz -C /opt/ibm
#   echo -e "[Db2]\nDescription = IBM Db2 ODBC Driver\nDriver = /opt/ibm/clidriver/lib/libdb2.so" \
#     | sudo tee /etc/odbcinst.ini
#
# After setup, run the integration tests:
#   DB2_TEST=1 cargo test --test integration -- --test-threads=1
# ================================================================
set -euo pipefail

CONTAINER_NAME="db2-test"
DB_NAME="TESTDB"
DB_USER="db2inst1"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
MAX_WAIT="${DB2_MAX_WAIT:-600}"  # seconds

# Compose v2 (`docker compose`) with a fallback to the legacy v1 binary.
compose() {
    if docker compose version &>/dev/null; then
        docker compose "$@"
    else
        docker-compose "$@"
    fi
}

# ------------------------------------------------------------------
# Teardown
# ------------------------------------------------------------------
if [[ "${1:-}" == "teardown" ]]; then
    echo "Stopping and removing DB2 container..."
    cd "$PROJECT_DIR"
    compose down -v
    echo "Done."
    exit 0
fi

# ------------------------------------------------------------------
# Status
# ------------------------------------------------------------------
if [[ "${1:-}" == "status" ]]; then
    if docker inspect "$CONTAINER_NAME" &>/dev/null; then
        health=$(docker inspect --format='{{.State.Health.Status}}' "$CONTAINER_NAME" 2>/dev/null || echo "unknown")
        echo "Container $CONTAINER_NAME: running (health: $health)"
        docker exec "$CONTAINER_NAME" su - "$DB_USER" -c "db2 connect to $DB_NAME && db2 'SELECT COUNT(*) FROM TEST_SCHEMA.EMPLOYEES'" 2>/dev/null \
            && echo "Database is seeded and accessible." \
            || echo "Database may not be fully ready yet."
    else
        echo "Container $CONTAINER_NAME is not running."
    fi
    exit 0
fi

# ------------------------------------------------------------------
# Start
# ------------------------------------------------------------------
echo "==> Starting DB2 container..."
cd "$PROJECT_DIR"
compose up -d

echo "==> Waiting for DB2 to be ready (max ${MAX_WAIT}s)..."
elapsed=0
while ! docker exec "$CONTAINER_NAME" su - "$DB_USER" -c "db2 connect to $DB_NAME" &>/dev/null; do
    if [[ $elapsed -ge $MAX_WAIT ]]; then
        echo "ERROR: DB2 did not become ready within ${MAX_WAIT}s"
        echo "--- Last 30 lines of container logs ---"
        docker logs "$CONTAINER_NAME" --tail 30
        exit 1
    fi
    sleep 5
    elapsed=$((elapsed + 5))
    printf "\r  ...waiting (%ds)" "$elapsed"
done
echo ""
echo "==> DB2 is ready!"

# Runs a fixture file inside the container. $1: file under tests/fixtures,
# $2: db2 CLP options (statement terminator and verbosity).
run_fixture() {
    docker cp "$PROJECT_DIR/tests/fixtures/$1" "$CONTAINER_NAME":"/tmp/$1"
    docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
        "db2 connect to $DB_NAME >/dev/null && db2 $2 /tmp/$1"
}

# ------------------------------------------------------------------
# Reseed: drop the fixture schemas so they are loaded from scratch
# ------------------------------------------------------------------
if [[ "${1:-}" == "reseed" ]]; then
    echo "==> Dropping fixture schemas..."
    # Dropping a schema that does not exist is an error; that is fine here.
    run_fixture teardown.sql "-tf" >/dev/null || true
fi

# ------------------------------------------------------------------
# Check if already seeded
# ------------------------------------------------------------------
already_seeded=$(docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
    "db2 connect to $DB_NAME >/dev/null 2>&1 && db2 -x \"SELECT COUNT(*) FROM SYSCAT.TABLES WHERE TABSCHEMA='TEST_SCHEMA' AND TYPE='T'\" 2>/dev/null" \
    2>/dev/null | tr -d ' ' || echo "0")

if [[ "$already_seeded" -gt 0 ]]; then
    echo "==> TEST_SCHEMA already exists, skipping seed (use 'reseed' to reload the fixtures)."
else
    echo "==> Seeding database..."

    # Run table/data setup (semicolon-terminated)
    echo "  Creating tables, views, indexes, and test data..."
    run_fixture setup.sql "-tvf"

    # Run routines (@ terminated)
    echo "  Creating stored procedures and functions..."
    run_fixture routines.sql "-td@ -vf"

    # Add table and column comments (including multiline Unicode fixtures)
    echo "  Adding metadata comments..."
    run_fixture comments.sql "-td@ -vf"

    # Schemas, names and column types that exercise the catalog edge cases
    echo "  Creating edge-case schemas and tables..."
    run_fixture edge_cases.sql "-tvf"

    # Create explain tables for EXPLAIN PLAN support
    # SYSINSTALLOBJECTS fails when the tables are already there, which is
    # the case on a reseed: they live in SYSTOOLS, not in a fixture schema.
    echo "  Creating explain tables..."
    run_fixture explain_setup.sql "-tvf" || echo "  (explain tables already exist)"

    echo "==> Seed complete."
fi

echo ""
echo "============================================"
echo " DB2 test environment is ready"
echo "============================================"
echo " Host:     localhost"
echo " Port:     50000"
echo " Database: $DB_NAME"
echo " User:     $DB_USER"
echo " Password: db2test123"
echo " Schema:   TEST_SCHEMA"
echo ""
echo " Run integration tests:"
echo "   DB2_TEST=1 cargo test --test integration -- --test-threads=1"
echo ""
