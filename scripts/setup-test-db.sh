#!/usr/bin/env bash
# ================================================================
# Setup script for the DB2 integration test environment.
#
# Usage:
#   ./scripts/setup-test-db.sh          # start DB2 + seed data
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
#   echo -e "[Db2]\nDescription = IBM Db2 ODBC Driver\nDriver = /opt/ibm/clidriver/lib/libdb2o.so" \
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
MAX_WAIT=300  # seconds

# ------------------------------------------------------------------
# Teardown
# ------------------------------------------------------------------
if [[ "${1:-}" == "teardown" ]]; then
    echo "Stopping and removing DB2 container..."
    cd "$PROJECT_DIR"
    docker compose down -v 2>/dev/null || docker-compose down -v 2>/dev/null
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
docker-compose up -d 2>/dev/null || docker-compose up -d 2>/dev/null

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

# ------------------------------------------------------------------
# Check if already seeded
# ------------------------------------------------------------------
already_seeded=$(docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
    "db2 connect to $DB_NAME >/dev/null 2>&1 && db2 -x \"SELECT COUNT(*) FROM SYSCAT.TABLES WHERE TABSCHEMA='TEST_SCHEMA' AND TYPE='T'\" 2>/dev/null" \
    2>/dev/null | tr -d ' ' || echo "0")

if [[ "$already_seeded" -gt 0 ]]; then
    echo "==> TEST_SCHEMA already exists, skipping seed."
else
    echo "==> Seeding database..."

    # Copy fixtures into container
    docker cp "$PROJECT_DIR/tests/fixtures/setup.sql" "$CONTAINER_NAME":/tmp/setup.sql
    docker cp "$PROJECT_DIR/tests/fixtures/routines.sql" "$CONTAINER_NAME":/tmp/routines.sql
    docker cp "$PROJECT_DIR/tests/fixtures/comments.sql" "$CONTAINER_NAME":/tmp/comments.sql
    docker cp "$PROJECT_DIR/tests/fixtures/explain_setup.sql" "$CONTAINER_NAME":/tmp/explain_setup.sql

    # Run table/data setup (semicolon-terminated)
    echo "  Creating tables, views, indexes, and test data..."
    docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
        "db2 connect to $DB_NAME && db2 -tvf /tmp/setup.sql"

    # Run routines (@ terminated)
    echo "  Creating stored procedures and functions..."
    docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
        "db2 connect to $DB_NAME && db2 -td@ -vf /tmp/routines.sql"

    # Add table and column comments (including multiline Unicode fixtures)
    echo "  Adding metadata comments..."
    docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
        "db2 connect to $DB_NAME && db2 -td@ -vf /tmp/comments.sql"

    # Create explain tables for EXPLAIN PLAN support
    echo "  Creating explain tables..."
    docker exec "$CONTAINER_NAME" su - "$DB_USER" -c \
        "db2 connect to $DB_NAME && db2 -tvf /tmp/explain_setup.sql"

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
