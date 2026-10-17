# MCP Registry publication (maintainers)

`server.json` (repo root) is published to the official MCP Registry as `app.engramo/mcp` by the `mcp-registry`
job in `.github/workflows/release.yml`, after the GitHub release and npm publish succeed. Dry runs skip it.
The version fields in `server.json` are `0.0.0` placeholders; the job rewrites them (and the `.mcpb` URL tag and
`fileSha256`, read from the release's `engramo.mcpb.sha256` asset) into a staged copy.

The `app.engramo/*` namespace is proven by DNS (domain `engramo.app`). The npm package carries
`"mcpName": "app.engramo/mcp"`, which the registry checks against the published npm version.

Reference: https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/authentication.mdx

## One-time setup

1. Generate an Ed25519 key (OpenSSL 3+; macOS system LibreSSL lacks Ed25519, use `brew install openssl@3`):

   ```bash
   openssl genpkey -algorithm Ed25519 -out key.pem
   PUBLIC_KEY="$(openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64)"
   echo "engramo.app. IN TXT \"v=MCPv1; k=ed25519; p=${PUBLIC_KEY}\""
   PRIVATE_KEY="$(openssl pkey -in key.pem -noout -text | grep -A3 "priv:" | tail -n +2 | tr -d ' :\n')"
   ```

2. Add the printed TXT record on the **apex** of `engramo.app` (not under a selector such as `_mcp-auth`).
   Wait for propagation.
3. Create the GitHub environment `mcp-registry` (Settings > Environments), restrict deployment to tags
   (`v*`), and add one secret, `MCP_REGISTRY_PRIVATE_KEY`, holding `$PRIVATE_KEY` (hex). Do not add `NPM_TOKEN`.
4. Delete `key.pem` locally or keep it in a password manager.

The workflow logs in with
`mcp-publisher login dns --domain engramo.app --private-key "$MCP_REGISTRY_PRIVATE_KEY"`.

## Rotation

Generate a new key, **replace** the TXT record on the apex (a stale record is tried first and breaks
verification), then update the `MCP_REGISTRY_PRIVATE_KEY` secret.

## Bumping mcp-publisher

The job pins `MCP_PUBLISHER_VERSION` and `MCP_PUBLISHER_SHA256` (linux amd64 tarball). To bump, pick a release at
https://github.com/modelcontextprotocol/registry/releases and copy the `mcp-publisher_linux_amd64.tar.gz` line
from its `registry_<version>_checksums.txt`.
