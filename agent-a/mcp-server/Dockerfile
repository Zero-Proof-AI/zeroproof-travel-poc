# Multi-stage build for Agent A MCP Server (Rust)
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
COPY agent-a/Cargo.toml agent-a/Cargo.lock ./

# Copy workspace members
COPY agent-a/mcp-server/ ./mcp-server/
COPY agent-a/mcp-client/ ./mcp-client/
COPY agent-a/src/ ./src/

# Copy shared protocol
COPY zk-protocol/ ../zk-protocol/

# Build the MCP server in release mode
RUN cargo build --release -p agent-a-mcp-server

# Stage 2: Runtime
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the built binary from builder
COPY --from=builder /app/target/release/agent-a-mcp /app/agent-a-mcp

# Set environment variables with defaults

# Expose the MCP server port
EXPOSE 3029

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3029/ || exit 1

ENTRYPOINT ["/app/agent-a-mcp"]
