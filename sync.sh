#!/usr/bin/env bash
set -euo pipefail

plugin_src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$(uname -s)" in
  Linux*)
    PLUGINS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/tabularis/plugins"
    ;;
  Darwin*)
    PLUGINS_DIR="$HOME/Library/Application Support/com.debba.tabularis/plugins"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    PLUGINS_DIR="${APPDATA}/com.debba.tabularis/plugins"
    ;;
  *)
    echo "Unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

echo "Target plugins directory: $PLUGINS_DIR"

manifest="$plugin_src/.tabularium"
if [[ ! -f "$manifest" ]]; then
  echo ".tabularium not found in $plugin_src" >&2
  exit 1
fi

plugin_id=$(grep -o '"id"\s*:\s*"[^"]*"' "$manifest" | head -1 | sed 's/.*: *"\(.*\)"/\1/')
executable=$(grep -o '"executable"\s*:\s*"[^"]*"' "$manifest" | head -1 | sed 's/.*: *"\(.*\)"/\1/')

if [[ -z "$plugin_id" || -z "$executable" ]]; then
  echo "Could not parse .tabularium" >&2
  exit 1
fi

echo ""
echo "==> Plugin: $plugin_id"
echo "  Building (cargo build --release)..."
cargo build --release --manifest-path "$plugin_src/Cargo.toml"

dest_dir="$PLUGINS_DIR/$plugin_id"
mkdir -p "$dest_dir"

cp "$manifest" "$dest_dir/.tabularium"
echo "  Copied .tabularium"

bin_path="$plugin_src/target/release/$executable"
if [[ ! -f "$bin_path" && -f "$plugin_src/target/release/$executable.exe" ]]; then
  bin_path="$plugin_src/target/release/$executable.exe"
fi

if [[ ! -f "$bin_path" ]]; then
  echo "  [WARN] Executable '$executable' not found. Build may have failed." >&2
  exit 1
fi

cp "$bin_path" "$dest_dir/$(basename "$bin_path")"
chmod +x "$dest_dir/$(basename "$bin_path")" || true
echo "  Copied executable: $(basename "$bin_path")"
echo "  Installed to: $dest_dir"

echo ""
echo "Sync complete. Restart Tabularis to load the updated plugin."

