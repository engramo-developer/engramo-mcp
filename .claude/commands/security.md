Perform a security audit of $ARGUMENTS (or the entire codebase if none specified) for this Rust MCP server.

## Security Checklist

### API Token Handling (CRITICAL)
- [ ] `ENGRAM_API_TOKEN` never appears in log output — only structured tracing fields with sanitized values
- [ ] Token not echoed in tool output content (`CallToolResult`) or error messages
- [ ] Token not included in debug `{:?}` format of any publicly visible struct
- [ ] `McpConfig` does not implement `Display` with the raw token

### Input Validation
- [ ] All UUID parameters parsed with `Uuid::parse_str` via `parse_uuid()` — never interpolated raw into HTTP paths
- [ ] No user-controlled strings used in format strings that reach logs (use structured fields)
- [ ] Pagination parameters validated by type (i64, not raw string)

### Panic Safety (MCP-Critical)
- [ ] No `panic!` in any tool handler — a panic kills the MCP server process
- [ ] No `unwrap()` / `expect()` outside `main.rs` startup sequence
- [ ] No `unreachable!` in tool dispatch paths

### TLS & Network
- [ ] `reqwest::Client` built without `.danger_accept_invalid_certs(true)`
- [ ] No HTTP downgrade: base URL from `ENGRAM_API_URL` — production must use `https://`

### Quota / Permission Errors
- [ ] 402/429 quota errors returned as user-facing `is_error = true` messages — not panics
- [ ] 403 permission errors returned as `is_error = true` — not propagated as Rust `Err`
- [ ] Error messages do not leak internal implementation details

### Dependency Review
- [ ] `rmcp` version pinned — check for known CVEs
- [ ] `reqwest` version pinned
- [ ] No unused dependencies that increase attack surface

## Output Format

For each finding:
1. **File:Line** — exact location
2. **Severity** — `Critical` / `High` / `Medium` / `Low`
3. **Category** — (Token | Panic | Network | Input | Dependency)
4. **Finding** — what the vulnerability is
5. **Fix** — concrete remediation with code snippet

End with a summary table sorted by severity.

## Mandatory Post-Fix Steps

```bash
cargo fmt --all
cargo check
cargo clippy -- -D warnings
cargo test
```
