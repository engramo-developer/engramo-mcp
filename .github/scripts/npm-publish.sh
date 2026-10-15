#!/usr/bin/env bash
# Usage: npm-publish.sh <pkg_dir> [npm publish flags...]
#
# Idempotent, retrying `npm publish`, so a release run that died part-way
# (e.g. a Sigstore/Rekor hiccup) can be re-run without failing on packages
# that already went out.
#
# - A version already on the registry is skipped (npm never allows
#   republishing it). Skipped on --dry-run so a dry run still exercises pack.
# - A failed publish is retried. npm's own internal retry after a timed-out
#   Rekor upload reuses the same signature and gets a 409 "equivalent entry
#   already exists"; a fresh `npm publish` signs with a new ephemeral key, so
#   it doesn't. Before each retry the registry is re-checked in case the
#   failed attempt actually landed.
set -euo pipefail

pkg_dir="$1"
shift

name=$(node -p "require('./$pkg_dir/package.json').name")
version=$(node -p "require('./$pkg_dir/package.json').version")
spec="$name@$version"

dry_run=false
for flag in "$@"; do
  [ "$flag" = "--dry-run" ] && dry_run=true
done

is_published() {
  [ "$dry_run" = "false" ] && [ "$(npm view "$spec" version 2>/dev/null)" = "$version" ]
}

max_attempts=3
for attempt in $(seq 1 "$max_attempts"); do
  if is_published; then
    echo "$spec is already on the registry, skipping"
    exit 0
  fi
  if npm publish "$pkg_dir" "$@"; then
    exit 0
  fi
  if [ "$attempt" -lt "$max_attempts" ]; then
    echo "::warning::npm publish of $spec failed (attempt $attempt/$max_attempts), retrying"
    sleep $((attempt * 15))
  fi
done

# The last attempt may have landed despite reporting failure.
if is_published; then
  echo "$spec is on the registry despite the reported failure"
  exit 0
fi
echo "::error::npm publish of $spec failed after $max_attempts attempts"
exit 1
