#!/usr/bin/env bash
# Builds the ChatGPT plugin package uploaded on platform.openai.com/plugins.
# The icon is copied from mcpb/ and the version is taken from Cargo.toml, so
# neither is duplicated in git. Output: target/engramo-chatgpt-plugin.zip
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/chatgpt-plugin"
stage="$root/target/chatgpt-plugin"
out="$root/target/engramo-chatgpt-plugin.zip"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"
[ -n "$version" ] || { echo "version not found in Cargo.toml" >&2; exit 1; }

rm -rf "$stage" "$out"
mkdir -p "$stage/assets"
cp "$src/mcp.json" "$stage/"
cp "$root/mcpb/icon.png" "$stage/assets/icon.png"
VERSION="$version" python3 - "$src/plugin.json" "$stage/plugin.json" <<'PY'
import json, os, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
manifest["version"] = os.environ["VERSION"]
with open(sys.argv[2], "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2, ensure_ascii=False)
    f.write("\n")
PY

(cd "$stage" && zip -q -X -r "$out" plugin.json mcp.json assets)
echo "$out (version $version)"
