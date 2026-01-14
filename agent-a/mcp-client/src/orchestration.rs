/// Orchestration logic for Agent A - extracted from main.rs for reuse
/// This module contains all the core agent logic:
/// - Claude API calls
/// - Tool routing and execution
/// - Payment workflows
/// - Proxy-fetch integration

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use crate::proxy_fetch::{ZkfetchToolOptions, ToolOptionsMap, ProxyFetch, ProxyConfig};

/// Claude API request
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: i32,
    pub system: String,
    pub messages: Vec<ClaudeMessage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: String,
}

/// Claude API response
#[derive(Debug, Deserialize)]
pub struct ClaudeResponse {
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: String,
}

#[derive(Debug, Deserialize)]
pub struct ContentBlock {
    #[serde(default)]
    pub text: String,
}

/// Metadata tracking which fields were redacted from a proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionMetadata {
    /// Number of fields that were redacted
    pub redacted_field_count: usize,
    /// List of dot-notation paths that were redacted
    pub redacted_paths: Vec<String>,
    /// Whether redactions were applied (true if any fields were redacted)
    pub was_redacted: bool,
}

impl Default for RedactionMetadata {
    fn default() -> Self {
        Self {
            redacted_field_count: 0,
            redacted_paths: Vec::new(),
            was_redacted: false,
        }
    }
}

/// Cryptographic proof record for tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptographicProof {
    pub tool_name: String,
    pub timestamp: u64,
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub proof: serde_json::Value, // zkfetch proof
    pub proof_id: Option<String>,
    pub verified: bool,
    pub onchain_compatible: bool,
    
    /// Display version of response with sensitive fields redacted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_response: Option<serde_json::Value>,
    
    /// Metadata about which fields were redacted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_metadata: Option<RedactionMetadata>,
}

/// Booking state tracking across multi-turn conversations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingState {
    pub step: String, // "initial", "pricing", "passenger_name", "passenger_email", "payment_method", "enrollment_confirmation", "payment", "completed"
    pub from: String,
    pub to: String,
    pub price: f64,
    pub passenger_name: Option<String>,
    pub passenger_email: Option<String>,
    pub payment_method: Option<String>, // "visa", "other", etc.
    pub enrollment_token_id: Option<String>,
    pub instruction_id: Option<String>,
    pub vip: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cryptographic_traces: Vec<CryptographicProof>, // Collected proofs from agent-b calls
}

impl Default for BookingState {
    fn default() -> Self {
        Self {
            step: "initial".to_string(),
            from: String::new(),
            to: String::new(),
            price: 0.0,
            passenger_name: None,
            passenger_email: None,
            payment_method: None,
            enrollment_token_id: None,
            instruction_id: None,
            vip: false,
            cryptographic_traces: Vec::new(),
        }
    }
}

/// Agent configuration
pub struct AgentConfig {
    pub claude_api_key: String,
    pub server_url: String,
    pub payment_agent_url: Option<String>,
    pub payment_agent_enabled: bool,
    pub zkfetch_wrapper_url: Option<String>,
}

impl AgentConfig {
    pub fn from_env() -> Result<Self> {
        let claude_api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow!("ANTHROPIC_API_KEY environment variable not set"))?;
        
        let server_url = std::env::var("AGENT_A_SERVER_URL")
            .unwrap_or_else(|_| "http://localhost:3001".to_string());
        
        let payment_agent_url = std::env::var("PAYMENT_AGENT_URL").ok();
        let payment_agent_enabled = std::env::var("PAYMENT_AGENT_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase() == "true";
        
        let zkfetch_wrapper_url = std::env::var("ZKFETCH_WRAPPER_URL").ok();

        Ok(Self {
            claude_api_key,
            server_url,
            payment_agent_url,
            payment_agent_enabled,
            zkfetch_wrapper_url,
        })
    }
}

/// Build a map of tool-specific redaction rules for privacy-preserving proofs
/// 
/// This defines which sensitive fields should be masked in cryptographic proofs
/// for each MCP tool. The redaction rules use dot-notation paths to specify fields.
/// 
/// # Tool Redaction Rules
/// 
/// - **get-ticket-price**: No redactions (pricing is public info)
/// - **book-flight**: Masks passenger_name and passenger_email
/// - **enroll-card**: Masks card_number, cvv, expiry
/// - **initiate-purchase-instruction**: Masks amount, tokenId
/// - **retrieve-payment-credentials**: Masks tokenId, instructionId, credentials
pub fn build_tool_options_map() -> ToolOptionsMap {
    let mut map = HashMap::new();

    // get-ticket-price: Pricing query - no sensitive data
    // Pricing information is public and doesn't need redaction
    map.insert(
        "get-ticket-price".to_string(),
        ZkfetchToolOptions::default(),
    );

    // book-flight: Passenger booking - redact PII
    // Reveals ONLY: booking_id, confirmation_code, status
    // Hides: passenger_name, from, to, and other details
    let mut book_flight_paths = std::collections::HashMap::new();
    book_flight_paths.insert("booking_id".to_string(), "$.data.booking_id".to_string());
    
    map.insert(
        "book-flight".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive request body from proof
            // This keeps passenger PII out of the on-chain proof
            private_options: Some(json!({
                "hiddenParameters": ["passenger_name", "passenger_email"]
            })),
            // Select ONLY the fields we want to reveal - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.booking_id"}),
                json!({"jsonPath": "$.data.confirmation_code"}),
                json!({"jsonPath": "$.data.status"}),
            ]),
            response_redaction_paths: Some(book_flight_paths),
        },
    );

    // enroll-card: Payment card enrollment - redact card details
    // Reveals ONLY: tokenId
    // Hides: all card information from proof
    let mut enroll_card_paths = std::collections::HashMap::new();
    enroll_card_paths.insert("tokenId".to_string(), "$.data.tokenId".to_string());
    
    map.insert(
        "enroll-card".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive card data from proof
            private_options: Some(json!({
                "hiddenParameters": ["card_number", "cvv", "expiry"]
            })),
            // Select ONLY the token ID - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.tokenId"}),
            ]),
            response_redaction_paths: Some(enroll_card_paths),
        },
    );

    // initiate-purchase-instruction: Payment initiation - redact transaction details
    // Reveals ONLY: instructionId
    // Hides: amount, tokenId, and other sensitive transaction details from proof
    let mut purchase_paths = std::collections::HashMap::new();
    purchase_paths.insert("instructionId".to_string(), "$.data.instructionId".to_string());
    
    map.insert(
        "initiate-purchase-instruction".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive transaction data from proof
            private_options: Some(json!({
                "hiddenParameters": ["amount", "tokenId"]
            })),
            // Select ONLY the instruction ID - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.instructionId"}),
            ]),
            response_redaction_paths: Some(purchase_paths),
        },
    );

    // retrieve-payment-credentials: Payment credential retrieval - redact all sensitive data
    // Reveals ONLY: credentials
    // Hides: tokenId, instructionId, and other identifiers from proof
    let mut credentials_paths = std::collections::HashMap::new();
    credentials_paths.insert("credentials".to_string(), "$.data.credentials".to_string());
    
    map.insert(
        "retrieve-payment-credentials".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive identifiers from proof
            private_options: Some(json!({
                "hiddenParameters": ["tokenId", "instructionId"]
            })),
            // Select ONLY the credentials - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.credentials"}),
            ]),
            response_redaction_paths: Some(credentials_paths),
        },
    );

    map
}

/// Fetch tool definitions from a server with timeout
pub async fn fetch_tool_definitions(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<Value> {
    let url = format!("{}/tools", server_url);
    
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.get(&url).send()
    ).await {
        Ok(Ok(response)) => {
            if !response.status().is_success() {
                return Err(anyhow!("Server returned error status"));
            }
            response.json().await.map_err(|e| anyhow!("Failed to parse response: {}", e))
        }
        Ok(Err(e)) => Err(anyhow!("Network error: {}", e)),
        Err(_) => Err(anyhow!("Request timeout")),
    }
}

/// Fetch and merge tool definitions from all servers
pub async fn fetch_all_tools(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
) -> Result<Value> {
    let mut all_tools: Vec<Value> = Vec::new();
    
    // Skip Agent A tools if running in HTTP mode (localhost:3001) to avoid circular fetching
    let skip_agent_a = agent_a_url.contains("localhost:3001") || agent_a_url.contains("0.0.0.0:3001");
    
    if !skip_agent_a {
        // Fetch Agent A tools (optional - may not be available)
        match fetch_tool_definitions(client, agent_a_url).await {
            Ok(resp) => {
                if let Some(tools) = resp.get("tools").and_then(|t| t.as_array()) {
                    all_tools.extend(tools.clone());
                }
            }
            Err(_) => {
                eprintln!("Warning: Could not fetch Agent A tools from {}", agent_a_url);
            }
        }
    }
    
    // Fetch Agent B tools (required for travel bookings)
    match fetch_tool_definitions(client, agent_b_url).await {
        Ok(response) => {
            if let Some(tools) = response.get("tools").and_then(|t| t.as_array()) {
                all_tools.extend(tools.clone());
            }
        }
        Err(e) => {
            eprintln!("Warning: Could not fetch Agent B tools: {}", e);
            // Add fallback travel tools
            all_tools.push(json!({
                "name": "get-ticket-price",
                "description": "Get flight ticket pricing",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "vip": {"type": "boolean"}
                    }
                }
            }));
            all_tools.push(json!({
                "name": "book-flight",
                "description": "Book a flight",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "passenger_name": {"type": "string"},
                        "passenger_email": {"type": "string"}
                    }
                }
            }));
        }
    }
    
    // Fetch Payment Agent tools if available
    if let Some(payment_url) = payment_agent_url {
        match fetch_tool_definitions(client, payment_url).await {
            Ok(payment_response) => {
                let payment_tools = payment_response
                    .get("data")
                    .and_then(|d| d.get("tools"))
                    .or_else(|| payment_response.get("tools"))
                    .and_then(|t| t.as_array());
                
                if let Some(tools) = payment_tools {
                    all_tools.extend(tools.clone());
                }
            }
            Err(_) => {
                eprintln!("Warning: Could not fetch Payment Agent tools");
            }
        }
    }
    
    // If we have no tools at all, return defaults
    if all_tools.is_empty() {
        all_tools = vec![
            json!({
                "name": "get-ticket-price",
                "description": "Get flight ticket pricing",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "vip": {"type": "boolean"}
                    }
                }
            }),
            json!({
                "name": "book-flight",
                "description": "Book a flight",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "passenger_name": {"type": "string"},
                        "passenger_email": {"type": "string"}
                    }
                }
            }),
        ];
    }
    
    Ok(json!({ "tools": all_tools }))
}

/// Call Claude API to get tool recommendations
pub async fn call_claude(
    client: &reqwest::Client,
    config: &AgentConfig,
    user_query: &str,
    messages: &[ClaudeMessage],
    state: &BookingState,
    tool_definitions: &Value,
) -> Result<String> {
    let state_context = if state.step != "initial" {
        format!(
            "\n\nCURRENT BOOKING STATE:\n- Step: {}\n- From: {}\n- To: {}\n- Price: ${:.2}\n- Passenger: {}\n- Email: {}",
            state.step,
            state.from,
            state.to,
            state.price,
            state.passenger_name.as_deref().unwrap_or("Not provided"),
            state.passenger_email.as_deref().unwrap_or("Not provided")
        )
    } else {
        String::new()
    };

    let system = format!(
        r#"You are Agent A, an AI travel coordinator with payment capabilities.

You have access to these tools:
{}

When the user makes a request, analyze what tool(s) they need and provide a JSON response in this exact format:
{{
  "reasoning": "explanation of what you're doing",
  "tool_calls": [
    {{"name": "tool_name", "arguments": {{"param1": "value1", ...}}}}
  ],
  "user_message": "friendly message to the user explaining the action"
}}

TRAVEL & PRICING TOOLS (from Agent B MCP Server):
- For ticket pricing: use get-ticket-price
  - Requires: from, to, optional vip boolean
  - IMPORTANT: When user asks to book, ONLY suggest this tool first. Do NOT suggest book-flight yet.
- For flight booking: use book-flight
  - Requires: from, to, passenger_name, passenger_email
  - IMPORTANT: Do NOT suggest this. The AI will call this automatically after payment completes.

PAYMENT WORKFLOW:
1. When user requests booking:
   - ONLY suggest get-ticket-price first (with from, to, vip)
   - Do NOT suggest other tools yet
2. After user confirms and completes payment:
   - book-flight will be called automatically with passenger details
   - No need to suggest it

OTHER TOOLS:
- For formatting: use format_zk_input
- For proof generation: use request_attestation (inform user it takes 11-27 minutes)
- For verification: use verify_on_chain

PAYMENT TOOLS (if available):
- For card enrollment: use enroll-card
  - Requires: sessionId, consumerId, enrollmentReferenceId
- For payment initiation: use initiate-purchase-instruction
  - Requires: sessionId, consumerId, tokenId (from enroll-card), amount, merchant
- For retrieving credentials: use retrieve-payment-credentials
  - Requires: sessionId, consumerId, tokenId, instructionId (from initiate-purchase), transactionReferenceId

IMPORTANT:
- Only suggest tools that match the user's request
- Always use sessionId format: sess_<username> or sess_<uuid>
- For payment tools, use consumerId and enrollmentReferenceId from user context
- If unsure what to do, ask the user for clarification{}"#,
        tool_definitions.to_string(),
        state_context
    );

    // Reconstruct message history with current user message
    let mut all_messages = messages.to_vec();
    all_messages.push(ClaudeMessage {
        role: "user".to_string(),
        content: user_query.to_string(),
    });

    let request = ClaudeRequest {
        model: "claude-3-haiku-20240307".to_string(),
        max_tokens: 1024,
        system,
        messages: all_messages,
    };

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &config.claude_api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Claude API error: {}", error_text));
    }

    let claude_response: ClaudeResponse = response.json().await?;
    
    if let Some(content) = claude_response.content.first() {
        Ok(content.text.clone())
    } else {
        Err(anyhow!("No response from Claude"))
    }
}

/// Parse Claude's tool recommendations from JSON response
pub fn parse_tool_calls(claude_response: &str) -> Result<Vec<(String, Value)>> {
    let json_start = claude_response.find('{');
    let json_end = claude_response.rfind('}');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &claude_response[start..=end];
        let parsed: Value = serde_json::from_str(json_str)?;

        let mut tools = Vec::new();
        if let Some(tool_calls) = parsed.get("tool_calls").and_then(|t| t.as_array()) {
            for call in tool_calls {
                if let (Some(name), Some(args)) = (
                    call.get("name").and_then(|n| n.as_str()),
                    call.get("arguments"),
                ) {
                    tools.push((name.to_string(), args.clone()));
                }
            }
        }
        Ok(tools)
    } else {
        Err(anyhow!("Could not parse tool calls from Claude response"))
    }
}

/// Call any MCP server tool through zkfetch-wrapper to get cryptographic proof
pub async fn call_tool_with_proof(
    client: &reqwest::Client,
    server_url: &str,
    zkfetch_wrapper_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<(String, Option<CryptographicProof>)> {
    // If zkfetch-wrapper is configured, use ProxyFetch to get cryptographic proof
    if let Some(zkfetch_url) = zkfetch_wrapper_url {
        println!("[TOOL] Calling {} via zkfetch-wrapper (PROXIED)", tool_name);
        
        // Create ProxyFetch with zkfetch config
        let proxy_config = ProxyConfig {
            url: zkfetch_url.to_string(),
            proxy_type: "zkfetch".to_string(),
            username: None,
            password: None,
            tool_options_map: Some(build_tool_options_map()),
            default_zk_options: None,
            debug: std::env::var("DEBUG_PROXY_FETCH").is_ok(),
        };
        
        let proxy_fetch = ProxyFetch::new(proxy_config)?;
        let target_url = format!("{}/tools/{}", server_url, tool_name);
        
        // Add tool name to arguments so it can be extracted in proxy_fetch
        let mut arguments_with_name = arguments.clone();
        if let Some(obj) = arguments_with_name.as_object_mut() {
            obj.insert("name".to_string(), json!(tool_name));
        }
        
        // Use ProxyFetch which handles paramValues extraction in proxy_fetch.rs
        let response = proxy_fetch.post(&target_url, Some(arguments_with_name)).await?;
        
        // Extract proof from response
        let proof = response.get("proof").cloned();
        let verified = response.get("verified").and_then(|v| v.as_bool()).unwrap_or(false);
        let onchain_compatible = response.get("metadata")
            .and_then(|m| m.get("onchain_compatible"))
            .and_then(|o| o.as_bool())
            .unwrap_or(false);
        
        println!("[ZKFETCH] Received proof for tool: {}", tool_name);
        
        // Extract the tool response
        let tool_result = response.get("data").cloned().unwrap_or(json!({}));
        
        // Create cryptographic proof record
        let crypto_proof = if let Some(proof_data) = proof {
            // Note: Redactions are applied server-side by zkfetch-wrapper.
            // The proof returned is already on-chain verifiable with redactions applied.
            // We don't apply redactions locally - they happen at the zkfetch payload level.
            
            Some(CryptographicProof {
                tool_name: tool_name.to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                request: arguments.clone(),
                response: tool_result.clone(),
                proof: proof_data,
                proof_id: response.get("metadata")
                    .and_then(|m| m.get("proof_id"))
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string()),
                verified,
                onchain_compatible,
                display_response: Some(tool_result.clone()),
                redaction_metadata: None, // zkfetch-wrapper handles this server-side
            })
        } else {
            None
        };
        
        // Extract data and return
        if let Some(data) = tool_result.get("data") {
            println!("[PROOF] ✓ Proof collected for {} - Verified: {}, On-chain: {}", 
                     tool_name, verified, onchain_compatible);
            return Ok((data.to_string(), crypto_proof));
        } else {
            return Ok((tool_result.to_string(), crypto_proof));
        }
    }
    
    // Fallback: Direct call to Agent B without proof
    println!("[TOOL] Calling {} DIRECTLY (NO PROXY) - zkfetch-wrapper not configured", tool_name);
    let url = format!("{}/tools/{}", server_url, tool_name);

    let response = client
        .post(&url)
        .json(&arguments)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Server error: {}", error_text));
    }

    let result: Value = response.json().await?;

    if let Some(error) = result.get("error") {
        if error.is_null() {
            if let Some(data) = result.get("data") {
                println!("[TOOL] ✓ Direct call to {} succeeded (no proof collected)", tool_name);
                Ok((data.to_string(), None))
            } else {
                Err(anyhow!("Invalid server response"))
            }
        } else {
            Err(anyhow!("Tool error: {}", error))
        }
    } else if let Some(data) = result.get("data") {
        println!("[TOOL] ✓ Direct call to {} succeeded (no proof collected)", tool_name);
        Ok((data.to_string(), None))
    } else {
        Err(anyhow!("Invalid server response"))
    }
}

/// Call server tool via HTTP with optional zkfetch proof collection
pub async fn call_server_tool_with_proof(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
    zkfetch_wrapper_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<(String, Option<CryptographicProof>)> {
    let agent_b_tools = [
        "get-ticket-price",
        "book-flight",
    ];
    
    let payment_tools = [
        "enroll-card",
        "initiate-purchase-instruction",
        "retrieve-payment-credentials",
    ];
    
    if agent_b_tools.contains(&tool_name) {
        // Agent B tools - use zkfetch-wrapper if available to get proof
        return call_tool_with_proof(client, agent_b_url, zkfetch_wrapper_url, tool_name, arguments).await;
    }
    
    if payment_tools.contains(&tool_name) && payment_agent_url.is_some() {
        // Payment Agent tools - use zkfetch-wrapper if available to get proof
        return call_tool_with_proof(client, payment_agent_url.unwrap(), zkfetch_wrapper_url, tool_name, arguments).await;
    }
    
    // For non-Agent-B tools, use direct calls (backward compatibility)
    call_server_tool(client, agent_a_url, agent_b_url, payment_agent_url, tool_name, arguments)
        .await
        .map(|result| (result, None))
}

/// Call server tool via HTTP (routes to appropriate server: Agent A, Agent B, or Payment Agent)
pub async fn call_server_tool(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<String> {
    let agent_b_tools = [
        "get-ticket-price",
        "book-flight",
    ];
    
    let payment_tools = [
        "enroll-card",
        "initiate-purchase-instruction",
        "retrieve-payment-credentials",
    ];
    
    if agent_b_tools.contains(&tool_name) {
        // Agent B tools use direct HTTP calls
        let url = format!("{}/tools/{}", agent_b_url, tool_name);

        let response = client
            .post(&url)
            .json(&arguments)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Server error: {}", error_text));
        }

        let result: Value = response.json().await?;

        if let Some(error) = result.get("error") {
            if error.is_null() {
                if let Some(data) = result.get("data") {
                    Ok(data.to_string())
                } else {
                    Err(anyhow!("Invalid server response"))
                }
            } else {
                Err(anyhow!("Tool error: {}", error))
            }
        } else if let Some(data) = result.get("data") {
            Ok(data.to_string())
        } else {
            Err(anyhow!("Invalid server response"))
        }
    } else if payment_tools.contains(&tool_name) && payment_agent_url.is_some() {
        // Payment Agent tools
        let payment_url = payment_agent_url.unwrap();
        let url = format!("{}/tools/{}", payment_url, tool_name);

        let response = client
            .post(&url)
            .json(&arguments)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Payment agent error: {}", error_text));
        }

        let result: Value = response.json().await?;

        if let Some(error) = result.get("error") {
            if error.is_null() {
                if let Some(data) = result.get("data") {
                    Ok(data.to_string())
                } else {
                    Err(anyhow!("Invalid server response"))
                }
            } else {
                Err(anyhow!("Tool error: {}", error))
            }
        } else if let Some(data) = result.get("data") {
            Ok(data.to_string())
        } else {
            Err(anyhow!("Invalid server response"))
        }
    } else {
        // Agent A tools or other tools
        let url = format!("{}/tools/{}", agent_a_url, tool_name);

        let response = client
            .post(&url)
            .json(&arguments)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Server error: {}", error_text));
        }

        let result: Value = response.json().await?;

        if let Some(error) = result.get("error") {
            if error.is_null() {
                if let Some(data) = result.get("data") {
                    Ok(data.to_string())
                } else {
                    Err(anyhow!("Invalid server response"))
                }
            } else {
                Err(anyhow!("Tool error: {}", error))
            }
        } else if let Some(data) = result.get("data") {
            Ok(data.to_string())
        } else {
            Err(anyhow!("Invalid server response"))
        }
    }
}

/// Complete a flight booking with payment processing
pub async fn complete_booking_with_payment(
    config: &AgentConfig,
    session_id: &str,
    from: &str,
    to: &str,
    price: f64,
    passenger_name: &str,
    passenger_email: &str,
    state: &mut BookingState,
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    let session_id = session_id.to_string();
    let mut enrollment_token_id = "token_789".to_string();
    let mut enrollment_complete = false;

    // Step 1: Check if card is already enrolled
    if let Some(payment_url) = payment_agent_url {
        let session_url = format!("{}/session/{}", payment_url, session_id);
        
        if let Ok(response) = client.get(&session_url).send().await {
            if let Ok(session_data) = response.json::<Value>().await {
                if let Some(data) = session_data.get("data") {
                    if let Some(token_count) = data.get("enrolledTokenCount").and_then(|c| c.as_u64()) {
                        if token_count > 0 {
                            enrollment_complete = true;
                            if let Some(token_ids) = data.get("enrolledTokenIds").and_then(|ids| ids.as_array()) {
                                if let Some(first_token) = token_ids.first().and_then(|t| t.as_str()) {
                                    enrollment_token_id = first_token.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Step 2: Enroll card if needed
    if !enrollment_complete && payment_agent_url.is_some() {
        let enroll_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "enrollmentReferenceId": "enroll_ref_456"
        });

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "enroll-card",
            enroll_args,
        )
        .await
        {
            Ok(result) => {
                if let Ok(parsed) = serde_json::from_str::<Value>(&result) {
                    let is_success = parsed.get("success").and_then(|s| s.as_bool()).unwrap_or(false) ||
                        parsed.get("status").and_then(|s| s.as_str()).map(|s| s == "SUCCESS").unwrap_or(false);
                    
                    if is_success {
                        let token_id = parsed
                            .get("data")
                            .and_then(|data| data.get("tokenId"))
                            .or_else(|| parsed.get("tokenId"))
                            .and_then(|t| t.as_str());
                        
                        if let Some(token_id) = token_id {
                            enrollment_token_id = token_id.to_string();
                        }
                        enrollment_complete = true;
                    } else {
                        return Err(anyhow!("Card enrollment failed"));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("Card enrollment error: {}", e));
            }
        }
    }

    // Step 3: Initiate payment
    let mut instruction_id = String::new();
    if enrollment_complete && payment_agent_url.is_some() {
        let purchase_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "tokenId": enrollment_token_id,
            "amount": price.to_string(),
            "merchant": "ZeroProof Travel"
        });

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "initiate-purchase-instruction",
            purchase_args,
        )
        .await
        {
            Ok(result) => {
                if let Ok(purchase_response) = serde_json::from_str::<Value>(&result) {
                    if let Some(id) = purchase_response
                        .get("data")
                        .and_then(|data| data.get("instructionId"))
                        .or_else(|| purchase_response.get("instructionId"))
                        .and_then(|id| id.as_str())
                    {
                        instruction_id = id.to_string();
                    } else {
                        return Err(anyhow!("Could not extract instructionId from payment response"));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("Payment initiation error: {}", e));
            }
        }
    }

    // Step 4: Retrieve payment credentials
    if !instruction_id.is_empty() && payment_agent_url.is_some() {
        let retrieve_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "tokenId": enrollment_token_id,
            "instructionId": instruction_id,
            "transactionReferenceId": "txn_202"
        });

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "retrieve-payment-credentials",
            retrieve_args,
        )
        .await
        {
            Ok(_result) => {
                // Payment confirmed, continue to booking
            }
            Err(e) => {
                return Err(anyhow!("Payment credential retrieval error: {}", e));
            }
        }
    }

    // Step 5: Complete the flight booking
    let book_args = json!({
        "from": from,
        "to": to,
        "passenger_name": passenger_name,
        "passenger_email": passenger_email
    });

    match call_server_tool_with_proof(
        &client,
        &config.server_url,
        &agent_b_url,
        payment_agent_url,
        zkfetch_wrapper_url,
        "book-flight",
        book_args,
    )
    .await
    {
        Ok((result, proof)) => {
            // Collect and submit cryptographic proof if available
            if let Some(crypto_proof) = proof {
                state.cryptographic_traces.push(crypto_proof.clone());
                println!("[PROOF] Collected proof for book-flight: {}", state.cryptographic_traces.len());
                
                // Submit proof to database asynchronously
                let server_url = config.server_url.clone();
                let session_id_str = session_id.to_string();
                tokio::spawn(async move {
                    match submit_proof_to_database(&server_url, &session_id_str, &crypto_proof).await {
                        Ok(proof_id) => {
                            println!("[PROOF] Submitted proof to database: {}", proof_id);
                        }
                        Err(e) => {
                            eprintln!("[PROOF] Failed to submit proof to database: {}", e);
                        }
                    }
                });
            }
            
            if let Ok(booking) = serde_json::from_str::<Value>(&result) {
                if let Some(conf_code) = booking.get("confirmation_code").and_then(|c| c.as_str()) {
                    return Ok(format!(
                        "🎉 Flight Booking Confirmed!\n\nConfirmation Code: {}\n\nYour flight from {} to {} has been successfully booked for {}.\n\nTotal Cost: ${:.2}\n\nA detailed confirmation email has been sent to {}.\n\nYour payment has been securely processed using biometric authentication.",
                        conf_code, from, to, passenger_name, price, passenger_email
                    ));
                }
            }
            Ok(format!("Booking completed. Result: {}", result))
        }
        Err(e) => Err(anyhow!("Failed to complete booking: {}", e)),
    }
}

/// Process a user query through the full orchestration pipeline
/// Handles multi-turn conversations including booking workflows
/// Returns (response_text, updated_messages, updated_state)
pub async fn process_user_query(
    config: &AgentConfig,
    user_query: &str,
    messages: &[ClaudeMessage],
    state: &mut BookingState,
    session_id: &str,
) -> Result<(String, Vec<ClaudeMessage>, BookingState)> {
    let client = reqwest::Client::new();

    // Fetch tool definitions
    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());
    
    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };
    
    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    let tool_definitions = fetch_all_tools(&client, &config.server_url, &agent_b_url, payment_agent_url).await?;

    // Call Claude with full message history
    let claude_response = call_claude(&client, config, user_query, messages, state, &tool_definitions).await?;

    // Build updated message list
    let mut updated_messages = messages.to_vec();
    
    // FIRST: Check state machine for controlled flow (before tool parsing)
    // This ensures the booking workflow progresses correctly regardless of Claude's response
    if state.step == "pricing" {
        // After pricing, ask for passenger name
        state.step = "passenger_name".to_string();
        let response = "Agent A: Great! I found your flight. To complete the booking, I'll need some information.\n\n📝 Step 1: Full Name\n\nWhat is your full name?".to_string();
        updated_messages.push(ClaudeMessage {
            role: "assistant".to_string(),
            content: response.clone(),
        });
        return Ok((response, updated_messages, state.clone()));
    }
    
    // Extract name and email using Claude's understanding
    // User provided name, now ask for email
    if state.step == "passenger_name" {
        let extracted_name = extract_with_claude(&client, config, "passenger_name", user_query, state, &tool_definitions).await?;
        
        if !extracted_name.is_empty() {
            state.passenger_name = Some(extracted_name.clone());
            state.step = "passenger_email".to_string();
            let response = format!(
                "Agent A: Perfect! Got it - {}.\n\n📧 Step 2: Email Address\n\nWhat is your email address?",
                extracted_name
            );
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            return Ok((response, updated_messages, state.clone()));
        } else {
            // Couldn't extract name, ask again
            let response = "Agent A: I couldn't understand that. Could you please provide your full name?".to_string();
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            return Ok((response, updated_messages, state.clone()));
        }
    }
    
    // User provided email, now ask for payment method
    if state.step == "passenger_email" {
        let extracted_email = extract_with_claude(&client, config, "passenger_email", user_query, state, &tool_definitions).await?;
        
        if !extracted_email.is_empty() {
            state.passenger_email = Some(extracted_email.clone());
            state.step = "payment_method".to_string();
            let passenger_name = state.passenger_name.clone().unwrap_or_default();
            
            let response = format!(
                "Agent A: Excellent! I have your details:\n- Name: {}\n- Email: {}\n\n💳 Step 3: Payment Method\n\nHow would you like to pay for this ${} flight?\n1. Visa Credit Card\n2. Other payment method\n\nPlease reply with 1 or 2.",
                passenger_name, extracted_email, state.price as i32
            );
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            return Ok((response, updated_messages, state.clone()));
        } else {
            // Couldn't extract email, ask again
            let response = "Agent A: I couldn't find a valid email. Please provide your email address (e.g., user@example.com):".to_string();
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            return Ok((response, updated_messages, state.clone()));
        }
    }
    
    // Parse tool calls
    match parse_tool_calls(&claude_response) {
        Ok(tool_calls) => {
            if tool_calls.is_empty() {
                // No tools needed, return Claude's response
                updated_messages.push(ClaudeMessage {
                    role: "assistant".to_string(),
                    content: claude_response.clone(),
                });
                Ok((format!("Agent A: {}", claude_response), updated_messages, state.clone()))
            } else {
                // Check if this is a pricing inquiry (get-ticket-price)
                let is_pricing_request = tool_calls.iter()
                    .any(|(name, _)| name == "get-ticket-price");

                if is_pricing_request {
                    // Execute pricing tool and return result with booking prompt
                    let mut pricing_result = None;
                    let mut from = String::new();
                    let mut to = String::new();
                    let mut price = 0.0;
                    
                    for (tool_name, arguments) in &tool_calls {
                        if tool_name == "get-ticket-price" {
                            if let Some(f) = arguments.get("from").and_then(|v| v.as_str()) {
                                from = f.to_string();
                            }
                            if let Some(t) = arguments.get("to").and_then(|v| v.as_str()) {
                                to = t.to_string();
                            }

                            match call_server_tool_with_proof(
                                &client,
                                &config.server_url,
                                &agent_b_url,
                                payment_agent_url,
                                zkfetch_wrapper_url,
                                tool_name,
                                arguments.clone(),
                            )
                            .await
                            {
                                Ok((result, proof)) => {
                                    // Collect and submit cryptographic proof if available
                                    if let Some(crypto_proof) = proof {
                                        state.cryptographic_traces.push(crypto_proof.clone());
                                        println!("[PROOF] Collected proof for {}: {}", tool_name, state.cryptographic_traces.len());
                                        
                                        // Submit proof to database asynchronously
                                        let server_url = config.server_url.clone();
                                        let session_id = session_id.to_string();
                                        tokio::spawn(async move {
                                            match submit_proof_to_database(&server_url, &session_id, &crypto_proof).await {
                                                Ok(proof_id) => {
                                                    println!("[PROOF] Submitted proof to database: {}", proof_id);
                                                }
                                                Err(e) => {
                                                    eprintln!("[PROOF] Failed to submit proof to database: {}", e);
                                                }
                                            }
                                        });
                                    }
                                    
                                    if let Ok(parsed) = serde_json::from_str::<Value>(&result) {
                                        if let Some(p) = parsed.get("price").and_then(|v| v.as_f64()) {
                                            price = p;
                                        }
                                    }
                                    pricing_result = Some(result);
                                }
                                Err(e) => {
                                    let err_msg = format!("Error fetching pricing: {}", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_msg.clone(),
                                    });
                                    return Ok((err_msg, updated_messages, state.clone()));
                                }
                            }
                        }
                    }

                    if let Some(_pricing) = pricing_result {
                        // Update state with pricing information
                        state.step = "pricing".to_string();
                        state.from = from.clone();
                        state.to = to.clone();
                        state.price = price;
                        
                        let response = format!(
                            "Agent A: Great! I found a flight from {} to {} for ${}.\n\nThis includes all taxes and fees.\n\nTo complete your booking, please provide:\n1. Your full name\n2. Your email address\n\nGive me your fullname first!",
                            from, to, price
                        );
                        
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        
                        return Ok((response, updated_messages, state.clone()));
                    }

                    updated_messages.push(ClaudeMessage {
                        role: "assistant".to_string(),
                        content: claude_response.clone(),
                    });
                    Ok((format!("Agent A: {}", claude_response), updated_messages, state.clone()))
                } else {
                    // Check if this is a booking confirmation with payment
                    let is_booking_with_payment = tool_calls.iter()
                        .any(|(name, _)| name == "enroll-card" || name == "initiate-purchase-instruction" || name == "book-flight");

                    if is_booking_with_payment {
                        // Payment method selection and enrollment
                        let payment_method = user_query.trim().to_lowercase();
                        
                        // User selected payment method. Ask for enrollment confirmation.
                        if state.step == "payment_method" {
                            // Check if user actually responded to payment method question
                            if !payment_method.contains("1") && !payment_method.contains("2") 
                                && !payment_method.contains("visa") && !payment_method.contains("other")
                                && !payment_method.contains("credit") && !payment_method.contains("card") {
                                // User didn't answer the payment method question clearly
                                let response = "Agent A: I need you to select your payment method. Please reply with:\n1. Visa Credit Card\n2. Other payment method".to_string();
                                updated_messages.push(ClaudeMessage {
                                    role: "assistant".to_string(),
                                    content: response.clone(),
                                });
                                return Ok((response, updated_messages, state.clone()));
                            }
                            
                            let selected_method = if payment_method.contains("1") || payment_method.contains("visa") {
                                "Visa Credit Card"
                            } else {
                                "Visa Credit Card" // Default to Visa if other selected
                            };
                            
                            // Update state with payment method
                            state.step = "enrollment_confirmation".to_string();
                            state.payment_method = Some(selected_method.to_string());
                            
                            let response = format!(
                                "Agent A: Perfect! You've selected {} for this transaction.\n\n🔐 Step 4: Biometric Authentication\n\nTo complete this booking, I'll need to enroll your payment card with biometric authentication.\n\nReady to proceed with payment enrollment? (Yes/No)",
                                selected_method
                            );
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            return Ok((response, updated_messages, state.clone()));
                        }
                        
                        // User confirmed enrollment. Now proceed with full payment (Turn 6)
                        if state.step == "enrollment_confirmation" {
                            // First check if user is responding to the enrollment confirmation prompt
                            let response_lower = user_query.trim().to_lowercase();
                            
                            if !response_lower.contains("yes") && !response_lower.contains("ok") && !response_lower.contains("confirm") && !response_lower.contains("proceed") && !response_lower.contains("y") {
                                // User didn't confirm, ask again
                                let response = "Agent A: I need your confirmation to proceed. Are you ready to proceed with payment enrollment? (Yes/No)".to_string();
                                updated_messages.push(ClaudeMessage {
                                    role: "assistant".to_string(),
                                    content: response.clone(),
                                });
                                return Ok((response, updated_messages, state.clone()));
                            }
                            
                            let from = state.from.clone();
                            let to = state.to.clone();
                            let price = state.price;
                            let passenger_name = state.passenger_name.clone().unwrap_or_default();
                            let passenger_email = state.passenger_email.clone().unwrap_or_default();

                            // Update state to payment
                            state.step = "payment".to_string();

                            match complete_booking_with_payment(
                                config,
                                session_id,
                                &from,
                                &to,
                                price,
                                &passenger_name,
                                &passenger_email,
                                state,
                            )
                            .await
                            {
                                Ok(result) => {
                                    state.step = "completed".to_string();
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: result.clone(),
                                    });
                                    Ok((result, updated_messages, state.clone()))
                                }
                                Err(e) => {
                                    let err_response = format!("Agent A: There was an issue processing your booking: {}\n\nPlease try again or contact support.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    Ok((err_response, updated_messages, state.clone()))
                                }
                            }
                        } else {
                            // Fallback: shouldn't reach here, but handle gracefully
                            let response = "Agent A: I'm ready to help with your booking. Could you please confirm your enrollment details?".to_string();
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            Ok((response, updated_messages, state.clone()))
                        }
                    } else {
                        // Non-pricing, non-booking tool flow - execute all tools
                        let mut results = Vec::new();
                        
                        for (tool_name, arguments) in &tool_calls {
                            match call_server_tool(
                                &client,
                                &config.server_url,
                                &agent_b_url,
                                payment_agent_url,
                                tool_name,
                                arguments.clone(),
                            )
                            .await
                            {
                                Ok(result) => {
                                    results.push(format!("Tool: {} | Result: {}", tool_name, result));
                                }
                                Err(e) => {
                                    results.push(format!("Tool: {} | Error: {}", tool_name, e));
                                }
                            }
                        }

                        // Extract user message from Claude response if available
                        let response = if let Ok(parsed) = serde_json::from_str::<Value>(&claude_response) {
                            if let Some(msg) = parsed.get("user_message").and_then(|m| m.as_str()) {
                                format!("Agent A: {}\n\nResults:\n{}", msg, results.join("\n"))
                            } else {
                                format!("Agent A: {}\n\nResults:\n{}", claude_response, results.join("\n"))
                            }
                        } else {
                            format!("Agent A: {}\n\nResults:\n{}", claude_response, results.join("\n"))
                        };
                        
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        
                        Ok((response, updated_messages, state.clone()))
                    }
                }
            }
        }
        Err(_) => {
            // Parse failed, return as conversational response
            let response = format!("Agent A: {}", claude_response);
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            Ok((response, updated_messages, state.clone()))
        }
    }
}

/// Helper: Extract information from user input using Claude's understanding
async fn extract_with_claude(
    client: &reqwest::Client,
    config: &AgentConfig,
    field: &str,
    user_input: &str,
    state: &BookingState,
    tool_definitions: &Value,
) -> Result<String> {
    let extraction_prompt = match field {
        "passenger_name" => format!(
            "Extract the passenger's full name from this user input: \"{}\"\n\nRespond with ONLY the name, nothing else. If no name is provided, respond with: NONE",
            user_input
        ),
        "passenger_email" => format!(
            "Extract the email address from this user input: \"{}\"\n\nRespond with ONLY the email address, nothing else. If no email is provided, respond with: NONE",
            user_input
        ),
        _ => return Ok(String::new()),
    };

    let claude_extraction = call_claude(
        client,
        config,
        &extraction_prompt,
        &[],
        state,
        tool_definitions,
    ).await.unwrap_or_default();

    let trimmed = claude_extraction.trim();
    if trimmed.to_uppercase() == "NONE" || trimmed.is_empty() {
        Ok(String::new())
    } else {
        Ok(trimmed.to_string())
    }
}
/// Submit a cryptographic proof to the Agent-A proof database with workflow metadata
pub async fn submit_proof_to_database(
    server_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
) -> Result<String> {
    submit_proof_to_database_with_metadata(
        server_url,
        session_id,
        proof,
        None,  // sequence - will be auto-assigned by database
        None,  // related_proof_id
        None,  // workflow_stage - will be inferred from tool_name
    ).await
}

/// Submit a proof with full workflow metadata
pub async fn submit_proof_to_database_with_metadata(
    server_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
    sequence: Option<u32>,
    related_proof_id: Option<String>,
    workflow_stage: Option<String>,
) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("{}/proofs", server_url);
    
    // Infer workflow_stage from tool_name if not provided
    let inferred_stage = workflow_stage.or_else(|| {
        match proof.tool_name.as_str() {
            "get-ticket-price" | "get-flight-options" => Some("pricing".to_string()),
            "enroll-card" => Some("payment_enrollment".to_string()),
            "create-payment-instruction" | "pay-for-ticket" => Some("payment".to_string()),
            "book-flight" => Some("booking".to_string()),
            _ => None,
        }
    });
    
    let mut payload = json!({
        "session_id": session_id,
        "tool_name": proof.tool_name,
        "timestamp": proof.timestamp,
        "request": proof.request,
        "response": proof.response,
        "proof": proof.proof,
        "proof_id": proof.proof_id,
        "verified": proof.verified,
        "onchain_compatible": proof.onchain_compatible,
        "submitted_by": "agent-a"
    });
    
    // Add optional workflow metadata
    if let Some(seq) = sequence {
        payload["sequence"] = json!(seq);
    }
    if let Some(ref rel_id) = related_proof_id {
        payload["related_proof_id"] = json!(rel_id);
    }
    if let Some(ref stage) = inferred_stage {
        payload["workflow_stage"] = json!(stage);
    }
    
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Failed to submit proof: {}", error_text));
    }
    
    let result: Value = response.json().await?;
    
    if let Some(proof_id) = result.get("proof_id").and_then(|p| p.as_str()) {
        Ok(proof_id.to_string())
    } else {
        Err(anyhow!("Invalid proof submission response"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_tool_options_map_contains_all_tools() {
        let tool_map = build_tool_options_map();
        
        // Verify all expected tools are in the map
        assert!(tool_map.contains_key("get-ticket-price"));
        assert!(tool_map.contains_key("book-flight"));
        assert!(tool_map.contains_key("enroll-card"));
        assert!(tool_map.contains_key("initiate-purchase-instruction"));
        assert!(tool_map.contains_key("retrieve-payment-credentials"));
    }

    #[test]
    fn test_build_tool_options_map_pricing_no_redactions() {
        let tool_map = build_tool_options_map();
        
        // get-ticket-price should have no redactions
        let pricing_opts = tool_map.get("get-ticket-price").unwrap();
        assert!(pricing_opts.redactions.is_none() || pricing_opts.redactions.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_build_tool_options_map_book_flight_redactions() {
        let tool_map = build_tool_options_map();
        
        // book-flight should reveal only booking confirmation details
        let booking_opts = tool_map.get("book-flight").unwrap();
        assert!(booking_opts.redactions.is_some());
        
        let redactions = booking_opts.redactions.as_ref().unwrap();
        assert!(!redactions.is_empty());
        
        // Verify response redaction paths (fields to reveal)
        assert!(booking_opts.response_redaction_paths.is_some());
        let paths = booking_opts.response_redaction_paths.as_ref().unwrap();
        assert!(paths.contains_key("booking_id"));
    }

    #[test]
    fn test_build_tool_options_map_enroll_card_redactions() {
        let tool_map = build_tool_options_map();
        
        // enroll-card should redact payment card details
        let enroll_opts = tool_map.get("enroll-card").unwrap();
        assert!(enroll_opts.redactions.is_some());
        
        let redactions = enroll_opts.redactions.as_ref().unwrap();
        assert!(redactions.len() >= 1);
        
        // Verify response redaction paths
        assert!(enroll_opts.response_redaction_paths.is_some());
        let paths = enroll_opts.response_redaction_paths.as_ref().unwrap();
        assert!(paths.contains_key("tokenId"));
    }

    #[test]
    fn test_build_tool_options_map_initiate_purchase_redactions() {
        let tool_map = build_tool_options_map();
        
        // initiate-purchase-instruction should redact transaction details
        let purchase_opts = tool_map.get("initiate-purchase-instruction").unwrap();
        assert!(purchase_opts.redactions.is_some());
        
        let redactions = purchase_opts.redactions.as_ref().unwrap();
        assert!(redactions.len() >= 1);
        
        // Verify response redaction paths
        assert!(purchase_opts.response_redaction_paths.is_some());
        let paths = purchase_opts.response_redaction_paths.as_ref().unwrap();
        assert!(paths.contains_key("instructionId"));
    }

    #[test]
    fn test_build_tool_options_map_retrieve_credentials_redactions() {
        let tool_map = build_tool_options_map();
        
        // retrieve-payment-credentials should reveal only credentials
        let retrieve_opts = tool_map.get("retrieve-payment-credentials").unwrap();
        assert!(retrieve_opts.redactions.is_some());
        
        let redactions = retrieve_opts.redactions.as_ref().unwrap();
        assert!(!redactions.is_empty());
        
        // Verify response redaction paths (fields to reveal)
        assert!(retrieve_opts.response_redaction_paths.is_some());
        let paths = retrieve_opts.response_redaction_paths.as_ref().unwrap();
        assert!(paths.contains_key("credentials"));
        
        // Verify private options hide sensitive request data
        assert!(retrieve_opts.private_options.is_some());
    }

    #[test]
    fn test_zkfetch_payload_includes_redactions() {
        // Verify that the zkfetch payload structure includes redactions for tools
        let tool_options_map = build_tool_options_map();
        
        // book-flight should have redactions
        let book_flight_opts = tool_options_map.get("book-flight");
        assert!(book_flight_opts.is_some(), "book-flight should be in tool options map");
        
        let book_flight = book_flight_opts.unwrap();
        assert!(book_flight.redactions.is_some(), "book-flight should have redactions");
        
        let redactions = book_flight.redactions.as_ref().unwrap();
        assert!(!redactions.is_empty(), "book-flight redactions should not be empty");
        
        // Verify redaction structure has required fields
        for redaction in redactions {
            assert!(redaction.get("jsonPath").is_some(), "Each redaction must have 'jsonPath' field");
        }
        
        // Verify that redactions can be serialized to JSON for zkfetch payload
        let payload = json!({
            "url": "http://agent-b/tools/book-flight",
            "publicOptions": {
                "method": "POST",
                "headers": {"Content-Type": "application/json"},
                "body": "{}"
            },
            "redactions": redactions
        });
        
        // Verify the payload structure is correct
        assert!(payload.get("url").is_some());
        assert!(payload.get("publicOptions").is_some());
        assert!(payload.get("redactions").is_some());
        
        // Verify redactions are properly nested in payload
        let payload_redactions = payload.get("redactions").unwrap();
        assert_eq!(payload_redactions.as_array().unwrap().len(), redactions.len());
    }

}