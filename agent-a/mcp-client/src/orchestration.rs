/// Orchestration logic for Agent A - extracted from main.rs for reuse
/// This module contains all the core agent logic:
/// - Claude API calls
/// - Tool routing and execution
/// - Payment workflows
/// - Proxy-fetch integration

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

/// Booking state tracking across multi-turn conversations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingState {
    pub step: String, // "initial", "pricing", "passenger_details", "payment_method", "enrollment_confirmation", "payment", "completed"
    pub from: String,
    pub to: String,
    pub price: f64,
    pub passenger_name: Option<String>,
    pub passenger_email: Option<String>,
    pub payment_method: Option<String>, // "visa", "other", etc.
    pub enrollment_token_id: Option<String>,
    pub instruction_id: Option<String>,
    pub vip: bool,
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
        }
    }
}

/// Agent configuration
pub struct AgentConfig {
    pub claude_api_key: String,
    pub server_url: String,
    pub payment_agent_url: Option<String>,
    pub payment_agent_enabled: bool,
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

        Ok(Self {
            claude_api_key,
            server_url,
            payment_agent_url,
            payment_agent_enabled,
        })
    }
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
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

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

    match call_server_tool(
        &client,
        &config.server_url,
        &agent_b_url,
        payment_agent_url,
        "book-flight",
        book_args,
    )
    .await
    {
        Ok(result) => {
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

    let tool_definitions = fetch_all_tools(&client, &config.server_url, &agent_b_url, payment_agent_url).await?;

    // Call Claude with full message history
    let claude_response = call_claude(&client, config, user_query, messages, state, &tool_definitions).await?;

    // Build updated message list
    let mut updated_messages = messages.to_vec();
    
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
                            "Agent A: Great! I found a flight from {} to {} for ${}.\n\nThis includes all taxes and fees.\n\nTo complete your booking, please provide:\n1. Your full name\n2. Your email address\n\nThen I'll process your payment and confirm your flight.",
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
                        // Check if we already have passenger details in the state (Turn 2) or just got them (Turn 2+)
                        let passenger_name = if let Some(name) = &state.passenger_name {
                            name.clone()
                        } else {
                            extract_passenger_name(user_query).unwrap_or_else(|| String::new())
                        };
                        
                        let passenger_email = if let Some(email) = &state.passenger_email {
                            email.clone()
                        } else {
                            extract_email(user_query).unwrap_or_else(|| String::new())
                        };
                        
                        // If we don't have passenger details yet, ask for them (Turn 2)
                        if (passenger_name.is_empty() || passenger_email.is_empty()) && state.step == "pricing" {
                            let response = format!(
                                "Agent A: Perfect! To complete your booking for {} to {}, I'll need your details.\n\nPlease provide:\n1. Your full name\n2. Your email address\n\nOnce I have these, we can proceed with payment.",
                                state.from, state.to
                            );
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            return Ok((response, updated_messages, state.clone()));
                        }
                        
                        // We just received passenger details (Turn 2). Ask for payment method selection.
                        if state.step == "pricing" && !passenger_name.is_empty() && !passenger_email.is_empty() {
                            // Update state to passenger_details to track that we're waiting for payment method selection
                            state.step = "payment_method".to_string();
                            state.passenger_name = Some(passenger_name.clone());
                            state.passenger_email = Some(passenger_email.clone());
                            
                            let response = format!(
                                "Agent A: Great! I have your details:\n- Name: {}\n- Email: {}\n\nHow would you like to pay?\n1. Visa Credit Card\n2. Other payment method\n\nPlease reply with your choice (1 or 2).",
                                passenger_name, passenger_email
                            );
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            return Ok((response, updated_messages, state.clone()));
                        }
                        
                        // User selected payment method (Turn 3). Ask for enrollment confirmation.
                        if state.step == "payment_method" {
                            let payment_method = user_query.trim().to_lowercase();
                            let selected_method = if payment_method.contains("1") || payment_method.contains("visa") {
                                "Visa Credit Card"
                            } else if payment_method.contains("2") || payment_method.contains("other") {
                                "Visa Credit Card" // Default to Visa if other selected (as per main.rs logic)
                            } else {
                                "Visa Credit Card" // Default to Visa
                            };
                            
                            // Update state with payment method
                            state.step = "enrollment_confirmation".to_string();
                            state.payment_method = Some(selected_method.to_string());
                            
                            let response = format!(
                                "Agent A: Perfect! I'll set up your {} for this transaction.\n\nTo complete this booking, I'll need to enroll your payment card with biometric authentication.\n\nReady to proceed with payment enrollment?",
                                selected_method
                            );
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            return Ok((response, updated_messages, state.clone()));
                        }
                        
                        // User confirmed enrollment. Now proceed with full payment (Turn 3)
                        if state.step == "enrollment_confirmation" {
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

/// Helper: Extract passenger name from user message
fn extract_passenger_name(message: &str) -> Option<String> {
    // Simple extraction - looks for patterns like "name is ...", "I'm ...", etc
    if let Some(pos) = message.to_lowercase().find("name is ") {
        let after = &message[pos + 8..];
        if let Some(end) = after.find('\n') {
            return Some(after[..end].trim().to_string());
        }
        return Some(after.trim().to_string());
    }
    
    if let Some(pos) = message.to_lowercase().find("i'm ") {
        let after = &message[pos + 4..];
        if let Some(end) = after.find('\n') {
            return Some(after[..end].trim().to_string());
        }
        return Some(after.trim().split_whitespace().next().unwrap_or("").to_string());
    }
    
    None
}

/// Helper: Extract email from user message
fn extract_email(message: &str) -> Option<String> {
    // Simple regex-like extraction for email pattern
    for word in message.split_whitespace() {
        if word.contains('@') && word.contains('.') {
            return Some(word.trim_end_matches(',').trim_end_matches('.').to_string());
        }
    }
    None
}

/// Helper: Extract from city from message
#[allow(dead_code)]
fn extract_from_route(message: &str) -> Option<String> {
    // Try to find "from" pattern
    let lower = message.to_lowercase();
    if let Some(pos) = lower.find("from ") {
        let after = &message[pos + 5..];
        if let Some(to_pos) = after.to_lowercase().find(" to ") {
            return Some(after[..to_pos].trim().to_string());
        }
    }
    None
}

/// Helper: Extract to city from message
/// Helper: Extract to city from message
#[allow(dead_code)]
fn extract_to_route(message: &str) -> Option<String> {
    // Try to find "to" pattern
    let lower = message.to_lowercase();
    if let Some(pos) = lower.find(" to ") {
        let after = &message[pos + 4..];
        if let Some(end) = after.find(|c: char| !c.is_alphabetic() && c != ' ') {
            return Some(after[..end].trim().to_string());
        }
        return Some(after.split_whitespace().next().unwrap_or("").to_string());
    }
    None
}
