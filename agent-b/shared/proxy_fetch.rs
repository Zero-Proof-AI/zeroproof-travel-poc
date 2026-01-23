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
//! use shared::proxy_fetch::{ProxyFetch, ProxyConfig, ZkfetchToolOptions};
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::proof::CryptographicProof;

/// Tool-specific ZK proof configuration for zkfetch-wrapper
#[derive(Debug, Clone)]
pub struct ZkfetchToolOptions {
    /// Options exposed in the generated ZK proof (e.g., method, headers)
    pub public_options: Option<Value>,

    /// Options hidden from the ZK proof (e.g., sensitive data)
    pub private_options: Option<Value>,

    /// Fields to exclude from the proof (specified as JSON paths)
    /// Example: `{"jsonPath": "$.data.card_number"}` to hide the card_number field
    pub redactions: Option<Vec<Value>>,

    /// Sensitive field paths in the response that should be redacted
    /// Maps field names to their jsonPath in the response structure
    /// Example: `{"passenger_name": "$.data.passenger_name"}`
    pub response_redaction_paths: Option<std::collections::HashMap<String, String>>,
}

impl Default for ZkfetchToolOptions {
    fn default() -> Self {
        Self {
            public_options: None,
            private_options: None,
            redactions: None,
            response_redaction_paths: None,
        }
    }
}

/// A redaction rule for masking sensitive data in proofs
///
/// Redactions use dot-notation paths to identify fields to mask.
/// Examples:
/// - `"response.data.passenger_name"` - masks passenger_name in response data
/// - `"body.card_number"` - masks card_number in request body
/// - `"request.body.cvv"` - masks CVV in request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionRule {
    /// Dot-notation path to the field to redact (e.g., "body.passenger_name")
    pub path: String,

    /// Type of redaction: "mask" (****), "hash", "remove"
    #[serde(default = "default_redaction_type")]
    pub redaction_type: String,
}

fn default_redaction_type() -> String {
    "mask".to_string()
}

/// Type alias for tool-specific redaction options
pub type ToolOptionsMap = HashMap<String, ZkfetchToolOptions>;

/// Apply redactions to a JSON value based on a list of redaction rules
///
/// Redactions are applied using dot-notation paths. Each redaction
/// masks the value at the specified path with "****".
///
/// # Arguments
/// * `value` - The JSON value to redact (modified in-place)
/// * `redactions` - List of redaction rules to apply
///
/// # Examples
/// ```rust,no_run
/// use serde_json::json;
///
/// let mut data = json!({
///     "name": "John Doe",
///     "email": "john@example.com"
/// });
///
/// let redactions = vec![
///     json!({"path": "name"}),
///     json!({"path": "email"}),
/// ];
///
/// apply_redactions(&mut data, &redactions);
/// assert_eq!(data["name"], "****");
/// assert_eq!(data["email"], "****");
/// ```
pub fn apply_redactions(value: &mut Value, redactions: &[Value]) {
    for redaction in redactions {
        if let Some(path) = redaction.get("path").and_then(|p| p.as_str()) {
            redact_at_path(value, path);
        }
    }
}

/// Redact a value at a specific dot-notation path
///
/// Navigates through nested objects using dot-separated path components
/// and masks the final value with "****".
///
/// # Arguments
/// * `value` - The root JSON value to navigate
/// * `path` - Dot-notation path (e.g., "response.data.passenger_name")
///
/// # Examples
/// ```rust,no_run
/// use serde_json::json;
///
/// let mut data = json!({
///     "response": {
///         "data": {
///             "passenger_name": "John Doe",
///             "booking_id": "BK123"
///         }
///     }
/// });
///
/// redact_at_path(&mut data, "response.data.passenger_name");
/// assert_eq!(data["response"]["data"]["passenger_name"], "****");
/// assert_eq!(data["response"]["data"]["booking_id"], "BK123");
/// ```
pub fn redact_at_path(value: &mut Value, path: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }

    // Navigate to the parent of the target field
    let mut current = value;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last component: redact it
            if let Some(obj) = current.as_object_mut() {
                if obj.contains_key(*part) {
                    obj.insert(part.to_string(), json!("****"));
                }
            }
        } else {
            // Intermediate component: navigate deeper
            if !current.is_object() {
                // If we hit a non-object before reaching the end, we can't navigate further
                return;
            }

            // Ensure the next level exists and navigate into it
            let obj = current.as_object_mut().unwrap();
            if !obj.contains_key(*part) {
                // Path doesn't exist, can't redact
                return;
            }

            current = &mut obj[*part];
        }
    }
}

/// Configuration for attestation service proof submission
///
/// When enabled, ProxyFetch will automatically extract ZK proofs from zkfetch responses
/// and submit them to the attestation service for storage and verification.
#[derive(Debug, Clone)]
pub struct AttestationConfig {
    /// URL of the attestation service (e.g., 'http://localhost:3001')
    pub service_url: String,

    /// Whether to automatically submit proofs to the attestation service
    pub enabled: bool,

    /// Optional workflow stage identifier (e.g., "pricing", "payment", "booking")
    /// If not provided, will be auto-inferred from the tool name
    pub workflow_stage: Option<String>,

    /// Unique session identifier for grouping related proofs
    /// If not provided, one will be generated per request
    pub session_id: Option<String>,

    /// Agent identifier for proof submission attribution (e.g., "agent-a" or "agent-b")
    pub submitted_by: String,
}

impl AttestationConfig {
    /// Creates a new attestation config for automatic proof submission
    pub fn new(service_url: String) -> Self {
        Self {
            service_url,
            enabled: true,
            workflow_stage: None,
            session_id: None,
            submitted_by: "unknown-agent".to_string(),
        }
    }

    /// Creates an attestation config with a specific workflow stage
    pub fn with_stage(service_url: String, workflow_stage: String) -> Self {
        Self {
            service_url,
            enabled: true,
            workflow_stage: Some(workflow_stage),
            session_id: None,
            submitted_by: "unknown-agent".to_string(),
        }
    }
}

impl Default for AttestationConfig {
    fn default() -> Self {
        Self {
            service_url: "https://dev.attester.zeroproofai.com".to_string(),
            enabled: true,
            workflow_stage: Some("general".to_string()),
            session_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
            submitted_by: "unknown-agent".to_string(),
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

    /// Optional attestation service configuration for automatic proof submission
    pub attestation_config: Option<AttestationConfig>,
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
            attestation_config: Some(AttestationConfig::default()),
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
/// use shared::proxy_fetch::{ProxyFetch, ProxyConfig};
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

        // Extract hidden parameters and convert to paramValues
        let mut final_body = body.unwrap_or(Value::Null);
        let mut private_options = zk_options.private_options.clone().unwrap_or_else(|| json!({}));
        let mut final_url = url.to_string();
        
        // Parse body if it's a string (likely from JSON serialization)
        if let Value::String(body_str) = &final_body {
            if let Ok(parsed) = serde_json::from_str::<Value>(body_str) {
                final_body = parsed;
            }
        }
        
        // Extract hidden parameters from body and URL
        let mut param_values_map = serde_json::Map::new();
        
        if let Some(private_opts) = private_options.as_object_mut() {
            if let Some(Value::Array(hidden_params)) = private_opts.get("hiddenParameters") {
                let hidden_params = hidden_params.clone();
                
                // Extract from request body
                if let Some(body_obj) = final_body.as_object_mut() {
                    for param in hidden_params.iter() {
                        if let Value::String(param_name) = param {
                            if let Some(value) = body_obj.remove(param_name) {
                                param_values_map.insert(param_name.clone(), value);
                                let placeholder = format!("{{{{{}}}}}", param_name);
                                body_obj.insert(param_name.clone(), Value::String(placeholder));
                            }
                        }
                    }
                }
                
                // Extract from URL query parameters
                if let Some(query_start) = final_url.find('?') {
                    let (base_url, query_string) = final_url.split_at(query_start);
                    let query_string = &query_string[1..]; // Skip the '?'
                    
                    let mut new_query_params = Vec::new();
                    let mut url_modified = false;
                    
                    for param_pair in query_string.split('&') {
                        if let Some((key, value)) = param_pair.split_once('=') {
                            // Check if this parameter is in hidden list
                            let is_hidden = hidden_params.iter().any(|p| {
                                if let Value::String(param_name) = p {
                                    param_name == key
                                } else {
                                    false
                                }
                            });
                            
                            if is_hidden {
                                // Decode the URL-encoded value
                                if let Ok(decoded_value) = urlencoding::decode(value) {
                                    param_values_map.insert(key.to_string(), Value::String(decoded_value.into_owned()));
                                    new_query_params.push(format!("{}={{{{{}}}}}", key, key));
                                    url_modified = true;
                                } else {
                                    new_query_params.push(param_pair.to_string());
                                }
                            } else {
                                new_query_params.push(param_pair.to_string());
                            }
                        } else {
                            new_query_params.push(param_pair.to_string());
                        }
                    }
                    
                    if url_modified {
                        final_url = format!("{}?{}", base_url, new_query_params.join("&"));
                    }
                }
                
                if !param_values_map.is_empty() {
                    private_opts.insert("paramValues".to_string(), Value::Object(param_values_map));
                    private_opts.remove("hideRequestBody");
                    private_opts.remove("hiddenParameters");
                }
            }
        }

        // Build zkfetch payload
        let zkfetch_payload = json!({
            "url": final_url,
            "publicOptions": {
                "method": method,
                "headers": {"Content-Type": "application/json"},
                "body": final_body.to_string(),
                "timeout": zk_options.public_options
                    .as_ref()
                    .and_then(|o| o.get("timeout").cloned())
                    .unwrap_or(json!(30000))
            },
            "privateOptions": private_options,
            "redactions": zk_options.redactions.unwrap_or_default()
        });

        if self.config.debug {
            tracing::info!("📦 Final zkfetch payload structure:");
            tracing::info!("  URL: {}", final_url);
            tracing::info!("  Method: {}", method);
            tracing::info!("  Public body: {}", zkfetch_payload.get("publicOptions").and_then(|o| o.get("body")).map(|b| b.to_string()).unwrap_or_default());
            
            // Log privateOptions structure
            if let Some(private_opts) = zkfetch_payload.get("privateOptions") {
                tracing::info!("  Private options keys: {:?}", private_opts.as_object().map(|o| o.keys().collect::<Vec<_>>()).unwrap_or_default());
                if let Some(pv) = private_opts.get("paramValues") {
                    tracing::info!("  ✓ paramValues present: {} keys", pv.as_object().map(|o| o.len()).unwrap_or(0));
                } else {
                    tracing::warn!("  ✗ paramValues NOT in privateOptions!");
                }
            }
            
            tracing::debug!("Full payload: {}", serde_json::to_string_pretty(&zkfetch_payload)?);
        }

        // POST to zkfetch-wrapper
        let zkfetch_url = format!("{}/zkfetch", self.config.url);
        let response = self
            .client
            .post(&zkfetch_url)
            .json(&zkfetch_payload)
            .send()
            .await?;

        let zkfetch_response = self.handle_response(response).await?;

        // If attestation is configured, extract proof and submit it asynchronously
        if let Some(attestation_config) = &self.config.attestation_config {
            println!("[PROXY_FETCH] attestation_config is Some, enabled={}", attestation_config.enabled);
            if attestation_config.enabled {
                println!("[PROXY_FETCH] Submitting proof for tool: {:?}", tool_name);
                self.submit_proof_async(
                    zkfetch_response.clone(),
                    tool_name.clone(),
                    attestation_config.clone(),
                    final_body.clone(),
                    final_url.clone(),
                ).await;
                println!("[PROXY_FETCH] ✓ Spawn call completed for tool: {:?}", tool_name);
            }
        } else {
            println!("[PROXY_FETCH] attestation_config is None!");
        }

        Ok(zkfetch_response)
    }

    /// Extracts proof from zkfetch response and submits it to attestation service asynchronously
    async fn submit_proof_async(
        &self,
        zkfetch_response: Value,
        tool_name: Option<String>,
        attestation_config: AttestationConfig,
        request_body: Value,
        request_url: String,
    ) {
        let tool_name_str = tool_name.unwrap_or_else(|| "unknown-tool".to_string());
        
        println!("[PROXY_FETCH] zkfetch_response for {}: {}", tool_name_str, serde_json::to_string_pretty(&zkfetch_response).unwrap_or_default());
        
        // Extract proof from zkfetch response
        // The proof is directly at response.proof, not response.proof.proof
        let proof_value = zkfetch_response
            .get("proof")
            .cloned();

        println!("[PROXY_FETCH] Extracted proof_value for {}: {:?}", tool_name_str, proof_value.is_some());
        
        if let Some(proof_json) = proof_value {
            // Generate session ID if not provided
            let session_id = attestation_config
                .session_id
                .clone()
                .unwrap_or_else(|| {
                    format!("agent-b-{}-{}", tool_name_str, chrono::Local::now().timestamp())
                });

            // Use provided workflow stage or default to "general"
            let workflow_stage = attestation_config
                .workflow_stage
                .clone()
                .or_else(|| Some("general".to_string()));

            // Spawn background task to submit proof
            let service_url = attestation_config.service_url.clone();
            let tool_name_clone = tool_name_str.clone();
            let response_body = zkfetch_response.clone();
            
            tokio::spawn(async move {
                println!("[PROXY_FETCH] 🚀 Spawned task started for: {}", tool_name_clone);
                // Create CryptographicProof structure
                let crypto_proof = CryptographicProof {
                    tool_name: tool_name_clone.clone(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    request: json!({
                        "url": request_url,
                        "body": request_body,
                    }),
                    response: response_body,
                    proof: proof_json,
                    proof_id: None,
                    verified: true,  // zkfetch proofs are cryptographically verified
                    onchain_compatible: true,  // ZK-TLS proofs can be verified on-chain
                    display_response: None,
                    redaction_metadata: None,
                };

                let client = reqwest::Client::new();
                match crate::submit_proof(
                    &client,
                    &service_url,
                    &session_id,
                    &crypto_proof,
                    workflow_stage,
                    &attestation_config.submitted_by,
                ).await {
                    Ok(proof_id) => {
                        tracing::info!(
                            "[PROXY_FETCH] ✓ Proof submitted to attestation service for {}: {}",
                            tool_name_clone, proof_id
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[PROXY_FETCH] Failed to submit proof for {}: {}",
                            tool_name_clone, e
                        );
                    }
                }
            });
        } else {
            tracing::warn!("[PROXY_FETCH] No proof received from zkfetch for tool: {}", tool_name_str);
        }
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
            response_redaction_paths: None,
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
            response_redaction_paths: None,
        };

        let config = ProxyConfig {
            default_zk_options: Some(default_options.clone()),
            ..Default::default()
        };
        let proxy = ProxyFetch::new(config).unwrap();

        let resolved = proxy.resolve_tool_options(&Some("unknown-tool".to_string()));
        assert_eq!(resolved.public_options, Some(json!({"timeout": 15000})));
    }

    #[test]
    fn test_redact_at_path_simple() {
        let mut data = json!({
            "name": "John Doe",
            "email": "john@example.com",
            "id": "123"
        });

        redact_at_path(&mut data, "name");
        assert_eq!(data["name"], "****");
        assert_eq!(data["email"], "john@example.com");
        assert_eq!(data["id"], "123");
    }

    #[test]
    fn test_redact_at_path_nested() {
        let mut data = json!({
            "response": {
                "data": {
                    "passenger_name": "John Doe",
                    "booking_id": "BK123",
                    "confirmation_code": "CONF456"
                }
            }
        });

        redact_at_path(&mut data, "response.data.passenger_name");
        assert_eq!(data["response"]["data"]["passenger_name"], "****");
        assert_eq!(data["response"]["data"]["booking_id"], "BK123");
        assert_eq!(data["response"]["data"]["confirmation_code"], "CONF456");
    }

    #[test]
    fn test_redact_at_path_nonexistent() {
        let mut data = json!({
            "name": "John Doe"
        });

        // Should not panic, just return gracefully
        redact_at_path(&mut data, "nonexistent.field.path");
        assert_eq!(data["name"], "John Doe");
    }

    #[test]
    fn test_apply_redactions_multiple() {
        let mut data = json!({
            "request": {
                "body": {
                    "passenger_name": "John Doe",
                    "passenger_email": "john@example.com",
                    "from": "NYC",
                    "to": "LAX"
                }
            },
            "response": {
                "data": {
                    "booking_id": "BK123",
                    "confirmation_code": "CONF456"
                }
            }
        });

        let redactions = vec![
            json!({"path": "request.body.passenger_name"}),
            json!({"path": "request.body.passenger_email"}),
        ];

        apply_redactions(&mut data, &redactions);

        assert_eq!(data["request"]["body"]["passenger_name"], "****");
        assert_eq!(data["request"]["body"]["passenger_email"], "****");
        assert_eq!(data["request"]["body"]["from"], "NYC");
        assert_eq!(data["request"]["body"]["to"], "LAX");
        assert_eq!(data["response"]["data"]["booking_id"], "BK123");
    }

    #[test]
    fn test_apply_redactions_payment_fields() {
        // Simulate a payment enrollment proof
        let mut data = json!({
            "request": {
                "body": {
                    "card_number": "4111111111111111",
                    "cvv": "123",
                    "expiry": "12/25",
                    "card_holder": "John Doe"
                }
            },
            "response": {
                "tokenId": "token_abc123",
                "status": "success"
            }
        });

        let redactions = vec![
            json!({"path": "request.body.card_number"}),
            json!({"path": "request.body.cvv"}),
            json!({"path": "request.body.expiry"}),
        ];

        apply_redactions(&mut data, &redactions);

        assert_eq!(data["request"]["body"]["card_number"], "****");
        assert_eq!(data["request"]["body"]["cvv"], "****");
        assert_eq!(data["request"]["body"]["expiry"], "****");
        assert_eq!(data["request"]["body"]["card_holder"], "John Doe");
        assert_eq!(data["response"]["tokenId"], "token_abc123");
        assert_eq!(data["response"]["status"], "success");
    }
}
