#!/bin/sh
# Linux launcher for the Claude Desktop extension (.mcpb). The manifest can pick
# a command per OS but not per CPU arch, so Linux runs `/bin/sh` on this script,
# which execs the static musl binary for this machine. Shipped in the bundle as
# server/linux/engramo-mcp next to engramo-mcp-x64 and engramo-mcp-arm64.
set -eu
dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
case $(uname -m) in
  x86_64 | amd64) arch=x64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *)
    echo "engramo-mcp: unsupported CPU architecture '$(uname -m)' (need x86_64 or aarch64)" >&2
    exit 1
    ;;
esac
bin="$dir/engramo-mcp-$arch"
# Unzipping can drop the exec bit; restore it if we can (best effort).
[ -x "$bin" ] || chmod +x "$bin" 2>/dev/null || true
exec "$bin" "$@"
