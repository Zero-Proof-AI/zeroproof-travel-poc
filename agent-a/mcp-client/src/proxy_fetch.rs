//! Proxy Fetch Module - Route HTTP requests through a proxy server
//!
//! This module provides a `ProxyFetch` client that routes HTTP requests through either:
//! - Standard HTTP proxies with optional authentication
//! - zkfetch-wrapper for privacy-preserving ZK proof generation
//!
//! Supports tool-specific configuration for MCP tool calls, allowing different
//! tools to have different redaction policies and ZK options.
//!
//! # Examples
//!
//! ```rust,no_run
//! use mcp_client::proxy_fetch::{ProxyFetch, ProxyConfig, ZkfetchToolOptions};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create zkfetch proxy config
//!     let mut tool_options = std::collections::HashMap::new();
//!     
//!     let enroll_options = ZkfetchToolOptions {
//!         public_options: Some(json!({"timeout": 30000})),
//!         private_options: Some(json!({"providerId": "visa-enrollment"})),
//!         redactions: Some(vec![
//!             json!({"path": "body.pan"}),
//!             json!({"path": "body.cvv"}),
//!         ]),
//!     };
//!     tool_options.insert("enroll-card".to_string(), enroll_options);
//!
//!     let config = ProxyConfig {
//!         url: "http://localhost:8000".to_string(),
//!         proxy_type: "zkfetch".to_string(),
//!         username: None,
//!         password: None,
//!         tool_options_map: Some(tool_options),
//!         default_zk_options: None,
//!         debug: true,
//!     };
//!
//!     let proxy_fetch = ProxyFetch::new(config)?;
//!
//!     // Make a request through the proxy
//!     let response = proxy_fetch.post(
//!         "https://agent-b.example.com/tools",
//!         Some(json!({"name": "get-ticket-price", "params": {"route": "NYC-LAX"}}))
//!     ).await?;
//!
//!     println!("Response: {}", response);
//!     Ok(())
//! }
//! ```

use anyhow::{anyhow, Result};
use reqwest::{Client, RequestBuilder, Response};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Tool-specific ZK proof configuration for zkfetch-wrapper
#[derive(Debug, Clone)]
pub struct ZkfetchToolOptions {
    /// Options exposed in the generated ZK proof (e.g., method, headers)
    pub public_options: Option<Value>,

    /// Options hidden from the ZK proof (e.g., sensitive data)
    pub private_options: Option<Value>,

    /// Fields to exclude from the proof (specified as JSON paths)
    /// Example: `{"path": "body.card_number"}` to hide the card_number field
    pub redactions: Option<Vec<Value>>,
}

impl Default for ZkfetchToolOptions {
    fn default() -> Self {
        Self {
            public_options: None,
            private_options: None,
            redactions: None,
        }
    }
}

/// Configuration for proxy server routing
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy server URL (e.g., 'http://localhost:8000' for zkfetch-wrapper)
    pub url: String,

    /// Type of proxy: "http" (standard proxy) or "zkfetch" (privacy-preserving)
    pub proxy_type: String,

    /// Optional username for proxy authentication
    pub username: Option<String>,

    /// Optional password for proxy authentication
    pub password: Option<String>,

    /// Per-tool ZK options for different MCP tools
    pub tool_options_map: Option<HashMap<String, ZkfetchToolOptions>>,

    /// Default ZK options applied to all tools without specific config
    pub default_zk_options: Option<ZkfetchToolOptions>,

    /// Enable debug logging for proxy requests
    pub debug: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8080".to_string(),
            proxy_type: "http".to_string(),
            username: None,
            password: None,
            tool_options_map: None,
            default_zk_options: None,
            debug: false,
        }
    }
}

/// HTTP client that routes requests through a proxy server
///
/// Supports:
/// - Standard HTTP proxies with optional authentication
/// - zkfetch-wrapper for privacy-preserving ZK proof generation
/// - Tool-specific ZK options for different MCP tools
/// - Both sync and async operations
///
/// # Construction
///
/// ```rust,no_run
/// use mcp_client::proxy_fetch::{ProxyFetch, ProxyConfig};
///
/// let config = ProxyConfig {
///     url: "http://proxy.example.com:8080".to_string(),
///     proxy_type: "http".to_string(),
///     username: Some("user".to_string()),
///     password: Some("pass".to_string()),
///     ..Default::default()
/// };
///
/// let proxy_fetch = ProxyFetch::new(config)?;
/// ```
pub struct ProxyFetch {
    config: ProxyConfig,
    client: Client,
}

impl ProxyFetch {
    /// Creates a new ProxyFetch client with the given configuration
    ///
    /// # Arguments
    /// * `config` - ProxyConfig with proxy URL, type, and optional tool options
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be created
    pub fn new(config: ProxyConfig) -> Result<Self> {
        let client = Client::new();
        Ok(Self { config, client })
    }

    /// Makes a GET request through the proxy
    ///
    /// # Arguments
    /// * `url` - Target URL to request
    ///
    /// # Returns
    /// The response body as a JSON Value
    pub async fn get(&self, url: &str) -> Result<Value> {
        self.request(url, "GET", None).await
    }

    /// Makes a POST request through the proxy
    ///
    /// # Arguments
    /// * `url` - Target URL
    /// * `body` - Request body as JSON Value
    ///
    /// # Returns
    /// The response body as a JSON Value
    pub async fn post(&self, url: &str, body: Option<Value>) -> Result<Value> {
        self.request(url, "POST", body).await
    }

    /// Makes a PUT request through the proxy
    ///
    /// # Arguments
    /// * `url` - Target URL
    /// * `body` - Request body as JSON Value
    ///
    /// # Returns
    /// The response body as a JSON Value
    pub async fn put(&self, url: &str, body: Option<Value>) -> Result<Value> {
        self.request(url, "PUT", body).await
    }

    /// Makes a DELETE request through the proxy
    ///
    /// # Arguments
    /// * `url` - Target URL
    ///
    /// # Returns
    /// The response body as a JSON Value
    pub async fn delete(&self, url: &str) -> Result<Value> {
        self.request(url, "DELETE", None).await
    }

    /// Makes a generic HTTP request through the proxy
    ///
    /// This method:
    /// 1. Determines proxy type (HTTP or zkfetch)
    /// 2. For zkfetch: extracts tool name from request body
    /// 3. Resolves tool-specific ZK options
    /// 4. Routes request through appropriate proxy
    ///
    /// # Arguments
    /// * `url` - Target URL
    /// * `method` - HTTP method (GET, POST, PUT, DELETE, etc.)
    /// * `body` - Optional request body as JSON Value
    ///
    /// # Returns
    /// The response body as a JSON Value
    pub async fn request(&self, url: &str, method: &str, body: Option<Value>) -> Result<Value> {
        if self.config.proxy_type == "zkfetch" {
            self.zkfetch_request(url, method, body).await
        } else {
            self.http_proxy_request(url, method, body).await
        }
    }

    /// Routes a request through standard HTTP proxy
    ///
    /// # Arguments
    /// * `url` - Target URL
    /// * `method` - HTTP method
    /// * `body` - Optional request body
    ///
    /// # Returns
    /// The response body as a JSON Value
    async fn http_proxy_request(
        &self,
        url: &str,
        method: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        if self.config.debug {
            tracing::info!(
                "🔀 Routing through HTTP proxy: {} ({})",
                self.config.url,
                method
            );
        }

        let mut request = self.build_request(url, method, body)?;

        // Add proxy authentication if credentials provided
        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            use base64::Engine;
            let credentials = base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{}", username, password));
            request = request.header("Proxy-Authorization", format!("Basic {}", credentials));
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    /// Routes a request through zkfetch-wrapper for ZK proof generation
    ///
    /// This method:
    /// 1. Extracts tool name from request body (for tool-specific options)
    /// 2. Resolves per-tool ZK options or uses default
    /// 3. Builds zkfetch payload with public/private/redactions
    /// 4. POSTs to zkfetch-wrapper /zkfetch endpoint
    /// 5. Returns the verified response
    ///
    /// # Arguments
    /// * `url` - Target URL
    /// * `method` - HTTP method
    /// * `body` - Optional request body
    ///
    /// # Returns
    /// The response body as a JSON Value
    async fn zkfetch_request(
        &self,
        url: &str,
        method: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        // Extract tool name from request body for option resolution
        let tool_name = self.extract_tool_name(&body);

        if self.config.debug {
            tracing::info!(
                "🔐 Routing through zkfetch: {} (tool: {:?})",
                self.config.url,
                tool_name
            );
        }

        // Resolve tool-specific ZK options
        let zk_options = self.resolve_tool_options(&tool_name);

        // Build zkfetch payload
        let zkfetch_payload = json!({
            "url": url,
            "method": method,
            "publicOptions": {
                "headers": {"Content-Type": "application/json"},
                "body": body.unwrap_or(Value::Null).to_string(),
                "timeout": zk_options.public_options
                    .as_ref()
                    .and_then(|o| o.get("timeout").cloned())
                    .unwrap_or(json!(30000))
            },
            "privateOptions": zk_options.private_options.unwrap_or(Value::Null),
            "redactions": zk_options.redactions.unwrap_or_default()
        });

        if self.config.debug {
            tracing::debug!("zkfetch payload: {}", serde_json::to_string_pretty(&zkfetch_payload)?);
        }

        // POST to zkfetch-wrapper
        let zkfetch_url = format!("{}/zkfetch", self.config.url);
        let response = self
            .client
            .post(&zkfetch_url)
            .json(&zkfetch_payload)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// Builds a request builder for the given URL and method
    fn build_request(
        &self,
        url: &str,
        method: &str,
        body: Option<Value>,
    ) -> Result<RequestBuilder> {
        let request = match method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "DELETE" => self.client.delete(url),
            "PATCH" => self.client.patch(url),
            "HEAD" => self.client.head(url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        let request = request.header("Content-Type", "application/json");

        // Add body if provided
        let request = if let Some(body_value) = body {
            request.json(&body_value)
        } else {
            request
        };

        Ok(request)
    }

    /// Handles HTTP response and extracts JSON body
    async fn handle_response(&self, response: Response) -> Result<Value> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Proxy request failed with status {}: {}",
                status,
                error_text
            ));
        }

        let body = response.json::<Value>().await?;

        if self.config.debug {
            tracing::debug!("Response: {}", serde_json::to_string_pretty(&body)?);
        }

        Ok(body)
    }

    /// Extracts tool name from request body
    ///
    /// Looks for these patterns in order:
    /// 1. body.name (for MCP tools)
    /// 2. body.params.name (nested tool name)
    /// 3. Returns None if not found
    fn extract_tool_name(&self, body: &Option<Value>) -> Option<String> {
        body.as_ref().and_then(|b| {
            // Try direct name field
            if let Some(name) = b.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }

            // Try nested params.name
            if let Some(name) = b
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                return Some(name.to_string());
            }

            // Try params.toolName (snake_case variant)
            if let Some(name) = b
                .get("params")
                .and_then(|p| p.get("toolName"))
                .and_then(|n| n.as_str())
            {
                return Some(name.to_string());
            }

            None
        })
    }

    /// Resolves tool-specific ZK options
    ///
    /// Resolution order:
    /// 1. If tool_name is Some, looks up in tool_options_map
    /// 2. Falls back to default_zk_options if provided
    /// 3. Returns empty ZkfetchToolOptions if neither found
    fn resolve_tool_options(&self, tool_name: &Option<String>) -> ZkfetchToolOptions {
        if let Some(name) = tool_name {
            if let Some(options_map) = &self.config.tool_options_map {
                if let Some(options) = options_map.get(name) {
                    return options.clone();
                }
            }
        }

        self.config
            .default_zk_options
            .as_ref()
            .cloned()
            .unwrap_or_default()
    }

    /// Returns a reference to the proxy configuration
    pub fn config(&self) -> &ProxyConfig {
        &self.config
    }

    /// Returns a mutable reference to the HTTP client
    pub fn client_mut(&mut self) -> &mut Client {
        &mut self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_name_direct() {
        let config = ProxyConfig::default();
        let proxy = ProxyFetch::new(config).unwrap();

        let body = Some(json!({"name": "get-ticket-price", "params": {}}));
        assert_eq!(proxy.extract_tool_name(&body), Some("get-ticket-price".to_string()));
    }

    #[test]
    fn test_extract_tool_name_nested() {
        let config = ProxyConfig::default();
        let proxy = ProxyFetch::new(config).unwrap();

        let body = Some(json!({"params": {"name": "book-flight"}}));
        assert_eq!(proxy.extract_tool_name(&body), Some("book-flight".to_string()));
    }

    #[test]
    fn test_extract_tool_name_not_found() {
        let config = ProxyConfig::default();
        let proxy = ProxyFetch::new(config).unwrap();

        let body = Some(json!({"params": {}}));
        assert_eq!(proxy.extract_tool_name(&body), None);
    }

    #[test]
    fn test_resolve_tool_options_specific() {
        let mut tool_options = HashMap::new();
        let options = ZkfetchToolOptions {
            public_options: Some(json!({"timeout": 20000})),
            private_options: None,
            redactions: Some(vec![json!({"path": "body.sensitive"})]),
        };
        tool_options.insert("get-ticket-price".to_string(), options.clone());

        let config = ProxyConfig {
            tool_options_map: Some(tool_options),
            ..Default::default()
        };
        let proxy = ProxyFetch::new(config).unwrap();

        let resolved = proxy.resolve_tool_options(&Some("get-ticket-price".to_string()));
        assert_eq!(resolved.public_options, Some(json!({"timeout": 20000})));
    }

    #[test]
    fn test_resolve_tool_options_default() {
        let default_options = ZkfetchToolOptions {
            public_options: Some(json!({"timeout": 15000})),
            private_options: None,
            redactions: None,
        };

        let config = ProxyConfig {
            default_zk_options: Some(default_options.clone()),
            ..Default::default()
        };
        let proxy = ProxyFetch::new(config).unwrap();

        let resolved = proxy.resolve_tool_options(&Some("unknown-tool".to_string()));
        assert_eq!(resolved.public_options, Some(json!({"timeout": 15000})));
    }
}
