# Stage 1: Build the React app
FROM node:20-bookworm AS builder

WORKDIR /app

# Copy package files
COPY package*.json ./

# Install dependencies
RUN npm install --prefer-offline --no-audit

# Copy source code
COPY . .

# Build the React app
RUN npm run build

# Stage 2: Serve with nginx
FROM nginx:1.27-bookworm-perl

# Remove default nginx config
RUN rm /etc/nginx/conf.d/default.conf

# Create nginx config for React SPA (single page application)
RUN echo 'server { \
    listen 3000; \
    server_name localhost; \
    \
    root /usr/share/nginx/html; \
    index index.html; \
    \
    # Cache busting for static assets \
    location ~* \.(js|css|png|jpg|jpeg|gif|ico|svg|woff|woff2|ttf|eot)$ { \
        expires 1y; \
        add_header Cache-Control "public, immutable"; \
    } \
    \
    # Fallback to index.html for React routing \
    location / { \
        try_files $uri /index.html =404; \
    } \
}' > /etc/nginx/conf.d/default.conf

# Copy built app from builder stage
COPY --from=builder /app/build /usr/share/nginx/html

# Expose port 3000
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/ || exit 1

# Start nginx
CMD ["nginx", "-g", "daemon off;"]
