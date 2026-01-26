# Stage 1: Build the React app
FROM node:20-bookworm AS builder

# Copy the entire workspace structure to handle dependencies correctly
COPY . /workspace
WORKDIR /workspace/ai-agent

# Install dependencies and build
RUN npm install --prefer-offline --no-audit --legacy-peer-deps && npm run build

# Stage 2: Serve with nginx
FROM nginx:1.27-bookworm-perl

# Copy built app from builder stage
COPY --from=builder /workspace/ai-agent/build /usr/share/nginx/html

# Remove default config - will be mounted from ConfigMap
RUN rm -f /etc/nginx/conf.d/default.conf

# Expose port 3000
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/ || exit 1

# Override entrypoint to bypass nginx entrypoint.d scripts
ENTRYPOINT ["nginx", "-g", "daemon off;"]
