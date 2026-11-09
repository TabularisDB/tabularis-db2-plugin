#!/usr/bin/env bash
# ================================================================
# Fails when .tabularium declares a capability the Tabularium
# registry schema does not know.
#
# The registry's own validation accepts unknown capability keys and
# then silently strips them on ingestion, so a typo or a capability
# that only exists in a newer Tabularis would publish a manifest
# that differs from the one in this repo. This check keeps the two
# in step.
#
# Usage: ./scripts/check-capabilities.sh [path/to/.tabularium]
# ================================================================
set -euo pipefail

MANIFEST="${1:-$(cd "$(dirname "$0")/.." && pwd)/.tabularium}"
SCHEMA_URL="${TABULARIUM_SCHEMA_URL:-https://registry.tabularis.dev/manifest.schema.json?kind=driver}"

schema="$(curl -fsSL --retry 3 "$SCHEMA_URL")"

known="$(jq -r '
  [ .. | objects | select(has("properties")) | .properties
    | select(has("capabilities")) | .capabilities.properties // {} | keys[] ]
  | unique | .[]' <<<"$schema")"

if [[ -z "$known" ]]; then
    echo "ERROR: no capabilities found in the registry schema at $SCHEMA_URL" >&2
    exit 1
fi

unknown="$(jq -r '.capabilities | keys[]' "$MANIFEST" | grep -vxFf <(printf '%s\n' "$known") || true)"

if [[ -n "$unknown" ]]; then
    echo "ERROR: $MANIFEST declares capabilities the registry schema does not know:" >&2
    printf '  - %s\n' $unknown >&2
    exit 1
fi

echo "ok: all $(jq -r '.capabilities | length' "$MANIFEST") declared capabilities are known to the registry schema"
