# Multi-stage build for Agent B MCP Server (Rust)
# Stage 1: Builder
FROM rust:latest AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace and build files
COPY agent-b/Cargo.toml agent-b/Cargo.lock ./

# Copy workspace members
COPY agent-b/pricing-core/ ./pricing-core/
COPY agent-b/mcp-server/ ./mcp-server/
COPY agent-b/program/ ./program/
COPY agent-b/server/ ./server/

# Build the MCP server in release mode
RUN cargo build --release -p agent-b-mcp-server

# Stage 2: Runtime
FROM debian:bookworm-slim

WORKDIR /app

# Runtime container already has curl and needed utilities

# Copy the built binary from builder
COPY --from=builder /app/target/release/agent-b-mcp-server /app/agent-b-mcp-server

# Expose the MCP server port
EXPOSE 3030

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3030/health || exit 1

# Run the MCP server
CMD ["/app/agent-b-mcp-server"]
