# Multi-stage build for `engramo-mcp http` (Streamable HTTP mode, for remote
# clients such as ChatGPT — see CLAUDE.md and engram-ws/tdds/track3-mcp-chatgpt-app.md).
# `stdio` mode (Claude Desktop / Cursor) is not deployed via this image — that
# mode runs as a local process with ENGRAM_API_TOKEN, not a container.
#
# Mirrors the cargo-chef pattern used by ../engram-api/Dockerfile so dependency
# compilation is cached across builds.

# 1. Chef Stage: computes a recipe file to cache dependencies.
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

# 2. Planner Stage: analyzes Cargo.lock/toml to produce the dependency recipe.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# 3. Builder Stage: compiles dependencies (cached layer), then the binary.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin engramo-mcp

# 4. Runtime Stage: the actual tiny container we deploy.
FROM ubuntu:24.04 AS runtime
WORKDIR /app

# CA certificates are required for outbound HTTPS calls to the EngrAmo API
# (ENGRAM_API_URL) in prod.
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends ca-certificates \
    && apt-get autoremove -y \
    && apt-get clean -y \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/engramo-mcp /app/engramo-mcp

# Cloud Run routes traffic to the container's configured port (defaults to
# 8080 unless the service is configured otherwise); MCP_BIND_ADDR defaults to
# 0.0.0.0:8080 in engramo-mcp itself, so no PORT-forwarding shim is needed as
# long as the Cloud Run service's configured container port stays 8080.
# ENGRAM_API_URL is required; MCP_PUBLIC_URL must be set to this service's own
# public URL for .well-known/oauth-protected-resource to advertise a real
# value (see src/well_known.rs) — both are human/infra steps, not baked in here.
ENV MCP_BIND_ADDR=0.0.0.0:8080

ENTRYPOINT ["/app/engramo-mcp", "http"]
