/// Claude API integration module
/// Handles communication with the Claude API for tool recommendations

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::orchestration::{AgentConfig, BookingState, ClaudeMessage};
use crate::shared::proxy_fetch::{ProxyConfig, ProxyFetch, AttestationConfig};
use super::proof::CryptographicProof;
use super::tool_map::build_tool_options_map;

/// Claude API request
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: i32,
    pub system: String,
    pub messages: Vec<ClaudeMessage>,
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

/// Call Claude API to get tool recommendations
pub async fn call_claude(
    client: &reqwest::Client,
    config: &AgentConfig,
    user_query: &str,
    messages: &[ClaudeMessage],
    state: &BookingState,
    tool_definitions: &Value,
    custom_system_prompt: Option<&str>,
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

    let system = if let Some(custom_prompt) = custom_system_prompt {
        custom_prompt.to_string()
    } else {
        format!(
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
  - Requires: from (departure city code), to (destination city code), optional vip boolean
  - CRITICAL: Both 'from' AND 'to' must be provided by the user. If either is missing:
    * Ask the user to provide the missing information
    * Do NOT make up or assume a city code
    * Example: if user says "Find a ticket to Paris", ask "What city are you departing from?"
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
- If unsure what to do, ask the user for clarification
- NEVER assume or make up missing information - always ask the user{}"#,
            tool_definitions.to_string(),
            state_context
        )
    };

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

/// Call server tool via HTTP at a specific server URL
pub async fn call_server_tool(
    client: &reqwest::Client,
    server_url: &str,
    tool_name: &str,
    arguments: Value,
) -> Result<String> {
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

    // Extract data from response
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

/// Call any MCP server tool through zkfetch-wrapper to get cryptographic proof
pub async fn call_tool_with_proof(
    client: &reqwest::Client,
    server_url: &str,
    zkfetch_wrapper_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
    attestation_config: Option<AttestationConfig>,
) -> Result<(String, Option<CryptographicProof>)> {
    use serde_json::json;
    
    // If zkfetch-wrapper is configured, use ProxyFetch to get cryptographic proof
    if let Some(zkfetch_url) = zkfetch_wrapper_url {
        println!("[TOOL] Calling {} via zkfetch-wrapper (PROXIED)", tool_name);
        
        // Create ProxyFetch with zkfetch config
        // If attestation_config is provided, use it; otherwise let Default::default() kick in
        let mut proxy_config = ProxyConfig {
            url: zkfetch_url.to_string(),
            proxy_type: "zkfetch".to_string(),
            username: None,
            password: None,
            tool_options_map: Some(build_tool_options_map()),
            default_zk_options: None,
            debug: std::env::var("DEBUG_PROXY_FETCH").is_ok(),
            ..Default::default()  // Uses default attestation_config: Some(AttestationConfig::default())
        };
        
        // Override attestation_config if explicitly provided
        if let Some(config) = attestation_config {
            proxy_config.attestation_config = Some(config);
        }
        
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
        println!("[TOOL] Full response from {}: {}", tool_name, 
                 serde_json::to_string_pretty(&tool_result).unwrap_or_else(|_| tool_result.to_string()));
        
        if let Some(data) = tool_result.get("data") {
            println!("[PROOF] ✓ Proof collected for {} - Verified: {}, On-chain: {}", 
                     tool_name, verified, onchain_compatible);
            return Ok((data.to_string(), crypto_proof));
        } else {
            println!("[TOOL] No 'data' field found in response, returning full tool_result");
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
    server_url: &str,
    zkfetch_wrapper_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
    attestation_config: Option<AttestationConfig>,
) -> Result<(String, Option<CryptographicProof>)> {
    // Use zkfetch-wrapper if available to get proof
    if zkfetch_wrapper_url.is_some() {
        return call_tool_with_proof(client, server_url, zkfetch_wrapper_url, tool_name, arguments, attestation_config).await;
    }
    
    // Fallback: Direct call without proof
    call_server_tool(client, server_url, tool_name, arguments)
        .await
        .map(|result| (result, None))
}
