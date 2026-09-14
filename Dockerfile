# Build the Admin shell first so the Rust embedding boundary can include the
# compiled assets without putting them under a Vault content root.
FROM node:24-bookworm-slim AS frontend
WORKDIR /workspace/frontend/admin
RUN corepack enable && corepack prepare pnpm@11.19.0 --activate
COPY frontend/admin/package.json frontend/admin/pnpm-lock.yaml frontend/admin/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY frontend/admin/ ./
RUN pnpm build

FROM rust:1.94-bookworm AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY --from=frontend /workspace/frontend/admin/dist ./frontend/admin/dist
RUN cargo build --release --locked -p mcp-vault-server

FROM debian:bookworm-slim AS runtime
RUN groupadd --system mcpvault && useradd --system --gid mcpvault --home-dir /nonexistent --shell /usr/sbin/nologin mcpvault
RUN install -d -o mcpvault -g mcpvault /data
COPY --from=builder /workspace/target/release/mcp-vault /usr/local/bin/mcp-vault

ENV MCP_VAULT_DATA_DIR=/data \
    MCP_VAULT_SECRETS_DIR=/data/secrets \
    MCP_VAULT_DATA_BIND=0.0.0.0:8080 \
    MCP_VAULT_ADMIN_BIND=0.0.0.0:8081 \
    MCP_VAULT_BACKUP_DIR=/data/backups \
    MCP_VAULT_METRICS_ENABLED=false \
    MCP_VAULT_LOG_FORMAT=json

USER mcpvault
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8080 8081
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/mcp-vault"]
