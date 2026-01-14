/// Agent A - AI-powered MCP Client using Claude
///
/// This client:
/// 1. Takes user queries via stdin
/// 2. Calls Claude API to determine which tool to use and extract parameters
/// 3. Invokes the MCP server via HTTP with the appropriate tool
/// 4. Returns results to the user
///
/// Requires: ANTHROPIC_API_KEY environment variable (or in .env file)
/// Usage: mcp-client-ai (loads from .env or ANTHROPIC_API_KEY env var)

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

// Import from library
use mcp_client::proxy_fetch;

// Load .env file on startup
fn init_env() {
    let _ = dotenv::dotenv();
}

/// Claude API request
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: i32,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Debug, Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

/// Claude API response
#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    content: Vec<ContentBlock>,
    #[serde(default)]
    #[allow(dead_code)]
    stop_reason: String,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: String,
}

/// Agent configuration
struct AgentConfig {
    claude_api_key: String,
    server_url: String,
    payment_agent_url: Option<String>,
    payment_agent_enabled: bool,
}

impl AgentConfig {
    fn from_env() -> Result<Self> {
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

/// Fetch tool definitions from a server
async fn fetch_tool_definitions(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<Value> {
    let url = format!("{}/tools", server_url);
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Failed to fetch tools: {}", error_text));
    }

    let tools: Value = response.json().await?;
    Ok(tools)
}

/// Fetch and merge tool definitions from Agent A Server and Agent B MCP Server
async fn fetch_all_tools(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
) -> Result<Value> {
    // Fetch Agent A tools
    let mut all_tools: Vec<Value> = Vec::new();
    
    let agent_a_response = fetch_tool_definitions(client, agent_a_url).await;
    if let Ok(resp) = agent_a_response {
        if let Some(tools) = resp.get("tools").and_then(|t| t.as_array()) {
            all_tools.extend(tools.clone());
            println!("  [Agent A Server] Loaded {} tools", tools.len());
        }
    }
    
    // Fetch Agent B MCP Server tools
    match fetch_tool_definitions(client, agent_b_url).await {
        Ok(response) => {
            if let Some(tools) = response.get("tools").and_then(|t| t.as_array()) {
                all_tools.extend(tools.clone());
                println!("  [Agent B MCP Server] Loaded {} pricing/booking tools", tools.len());
            }
        }
        Err(e) => {
            println!("  ⚠️  Agent B MCP Server unavailable: {}", e);
            println!("     (Continuing with Agent A tools only)");
        }
    }
    
    // Fetch Payment Agent tools if available
    if let Some(payment_url) = payment_agent_url {
        match fetch_tool_definitions(client, payment_url).await {
            Ok(payment_response) => {
                // Payment Agent returns tools in data.tools
                let payment_tools = payment_response
                    .get("data")
                    .and_then(|d| d.get("tools"))
                    .or_else(|| payment_response.get("tools")) // fallback to direct "tools"
                    .and_then(|t| t.as_array());
                
                if let Some(tools) = payment_tools {
                    all_tools.extend(tools.clone());
                    println!("  [Payment Agent] Loaded {} payment tools", tools.len());
                }
            }
            Err(e) => {
                println!("  ⚠️  Payment Agent unavailable: {}", e);
                println!("     (Continuing without payment capabilities)");
            }
        }
    }
    
    Ok(json!({ "tools": all_tools }))
}

/// Call Claude API to get tool recommendations
async fn call_claude(
    client: &reqwest::Client,
    config: &AgentConfig,
    user_query: &str,
    tool_definitions: &Value,
) -> Result<String> {
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
- If unsure what to do, ask the user for clarification"#,
        tool_definitions.to_string()
    );

    let request = ClaudeRequest {
        model: "claude-3-haiku-20240307".to_string(),
        max_tokens: 1024,
        system,
        messages: vec![ClaudeMessage {
            role: "user".to_string(),
            content: user_query.to_string(),
        }],
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
fn parse_tool_calls(claude_response: &str) -> Result<Vec<(String, Value)>> {
    // Try to extract JSON from the response
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

/// Build tool options map for payment tools with appropriate redactions
fn build_payment_tool_options() -> std::collections::HashMap<String, proxy_fetch::ZkfetchToolOptions> {
    use std::collections::HashMap;
    
    let mut options_map = HashMap::new();
    
    // Redactions for sensitive payment fields
    let payment_redactions = vec![
        json!({"path": "body.card_number"}),
        json!({"path": "body.cvv"}),
        json!({"path": "body.expiry_date"}),
        json!({"path": "body.cardholder_name"}),
        json!({"path": "body.pin"}),
        json!({"path": "response.card_number"}),
        json!({"path": "response.cvv"}),
    ];
    
    // Enroll card tool - redact all PII and card details
    options_map.insert(
        "enroll-card".to_string(),
        proxy_fetch::ZkfetchToolOptions {
            public_options: Some(json!({"action": "enroll"})),
            private_options: None,
            redactions: Some(payment_redactions.clone()),
            response_redaction_paths: None,
        },
    );
    
    // Initiate purchase instruction - redact sensitive data
    options_map.insert(
        "initiate-purchase-instruction".to_string(),
        proxy_fetch::ZkfetchToolOptions {
            public_options: Some(json!({"action": "purchase"})),
            private_options: None,
            redactions: Some(payment_redactions.clone()),
            response_redaction_paths: None,
        },
    );
    
    // Retrieve payment credentials - redact all credentials
    options_map.insert(
        "retrieve-payment-credentials".to_string(),
        proxy_fetch::ZkfetchToolOptions {
            public_options: Some(json!({"action": "retrieve"})),
            private_options: None,
            redactions: Some(payment_redactions.clone()),
            response_redaction_paths: None,
        },
    );
    
    // Confirm transaction - redact sensitive transaction details
    options_map.insert(
        "confirm-transaction".to_string(),
        proxy_fetch::ZkfetchToolOptions {
            public_options: Some(json!({"action": "confirm"})),
            private_options: None,
            redactions: Some(payment_redactions),
            response_redaction_paths: None,
        },
    );
    
    options_map
}

/// Call server tool via HTTP (routes to appropriate server: Agent A, Agent B, or Payment Agent)
async fn call_server_tool(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<String> {
    // Determine which server to call based on tool name
    let payment_tools = [
        "enroll-card",
        "initiate-purchase-instruction",
        "retrieve-payment-credentials",
        "confirm-transaction",
    ];
    
    let agent_b_tools = [
        "get-ticket-price",
        "book-flight",
    ];
    
    // Use proxy_fetch for payment tools to enable zkfetch routing
    if payment_tools.contains(&tool_name) {
        if let Some(payment_url) = payment_agent_url {
            let proxy_config = proxy_fetch::ProxyConfig {
                url: std::env::var("PROXY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()),
                proxy_type: std::env::var("PROXY_TYPE").unwrap_or_else(|_| "direct".to_string()),
                username: std::env::var("PROXY_USERNAME").ok(),
                password: std::env::var("PROXY_PASSWORD").ok(),
                tool_options_map: Some(build_payment_tool_options()),
                default_zk_options: None,
                debug: std::env::var("DEBUG_PROXY_FETCH").is_ok(),
            };
            
            let proxy_fetch = proxy_fetch::ProxyFetch::new(proxy_config)?;
            
            let url = format!("{}/tools/{}", payment_url, tool_name);
            println!("[PAYMENT_PROXY_FETCH] Calling: {}", url);
            println!("[PAYMENT_PROXY_FETCH] Tool: {}", tool_name);
            println!("[PAYMENT_PROXY_FETCH] Arguments: {}", arguments);
            
            let response = proxy_fetch.post(&url, Some(arguments)).await?;
            println!("[PAYMENT_PROXY_FETCH] Response: {}", response);
            
            Ok(response.to_string())
        } else {
            return Err(anyhow!(
                "Tool '{}' requires Payment Agent, but PAYMENT_AGENT_URL not configured",
                tool_name
            ));
        }
    } else if agent_b_tools.contains(&tool_name) {
        // Agent B tools use direct HTTP calls for now
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
            // Check if error is not null
            if error.is_null() {
                // Error field exists but is null, check for data
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
        // Agent A tools use direct HTTP calls
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
            // Check if error is not null
            if error.is_null() {
                // Error field exists but is null, check for data
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

/// Helper: Ask user for confirmation (using pre-created stdin)
fn ask_confirmation_from_reader(question: &str, reader: &mut std::io::StdinLock, stdout: &mut std::io::Stdout) -> Result<bool> {
    loop {
        print!("{} [y/n] ", question);
        stdout.flush()?;
        
        let mut input = String::new();
        reader.read_line(&mut input)?;
        
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please answer 'y' or 'n'."),
        }
    }
}

/// Helper: Ask user for confirmation (legacy, creates new stdin)
#[allow(dead_code)]
fn ask_confirmation(question: &str) -> Result<bool> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    
    loop {
        print!("{} [y/n] ", question);
        stdout.flush()?;
        
        let mut input = String::new();
        stdin.read_line(&mut input)?;
        
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please answer 'y' or 'n'."),
        }
    }
}

/// Helper: Show status message
fn show_status(message: &str) {
    println!("\n⏳ {}", message);
    io::stdout().flush().ok();
}

/// Helper: Show success message
fn show_success(message: &str) {
    println!("\n✅ {}", message);
}

/// Helper: Show step indicator
fn show_step(step: u32, total: u32, message: &str) {
    println!("\n[Step {}/{}] {}", step, total, message);
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file
    init_env();
    
    let config = AgentConfig::from_env()?;
    let client = reqwest::Client::new();

    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║       Agent A - AI-Powered MCP Client (Claude)             ║");
    println!("║              (Connects to HTTP Server)                     ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    // Fetch tool definitions from servers
    println!("Fetching tool definitions...");
    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };
    
    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());
    
    let tool_definitions = match fetch_all_tools(&client, &config.server_url, &agent_b_url, payment_agent_url).await {
        Ok(tools) => {
            println!("✓ Loaded {} tools from server(s)\n", 
                tools.get("tools")
                    .and_then(|t| t.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0)
            );
            tools
        }
        Err(e) => {
            eprintln!("✗ Failed to fetch tools: {}\n", e);
            eprintln!("Make sure the MCP server is running on {}\n", config.server_url);
            return Err(e);
        }
    };

    println!("Capabilities:");
    if let Some(tools) = tool_definitions.get("tools").and_then(|t| t.as_array()) {
        for (i, tool) in tools.iter().enumerate() {
            if let Some(name) = tool.get("name").and_then(|n| n.as_str()) {
                if let Some(desc) = tool.get("description").and_then(|d| d.as_str()) {
                    println!("  {}. {} - {}", i + 1, name, desc);
                }
            }
        }
    }
    println!();

    println!("Examples:");
    println!("  'Get pricing from NYC to London for VIP'");
    println!("  'Verify a ZK proof on Sepolia'");
    println!("  'Request a ZK attestation'\n");

    println!("Type 'exit' or 'quit' to end.\n");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = stdin.lock();

    loop {
        print!("\nYou: ");
        stdout.flush()?;

        let mut user_input = String::new();
        if reader.read_line(&mut user_input)? == 0 {
            break; // EOF
        }
        
        let input = user_input.trim();

            if input.is_empty() {
                continue;
            }

            if matches!(input.to_lowercase().as_str(), "exit" | "quit") {
                println!("\nGoodbye!");
                break;
            }

            println!("\nAgent A: Processing your request...\n");

            // Call Claude to determine tools
            match call_claude(&client, &config, input, &tool_definitions).await {
                Ok(claude_response) => {
                    // Parse tool calls
                    match parse_tool_calls(&claude_response) {
                        Ok(tool_calls) => {
                            if tool_calls.is_empty() {
                                // No tools needed, just show Claude's response
                                println!("Agent A: {}\n", claude_response);
                            } else {
                                // Track if this is a payment flow (triggered by get-ticket-price tool)
                                let is_payment_flow = tool_calls.iter()
                                    .any(|(name, _)| name == "get-ticket-price");
                                
                                if is_payment_flow {
                                    // Interactive payment workflow
                                    show_step(1, 3, "Processing booking request...");
                                    
                                    // First tool (usually call_agent_b for pricing)
                                    let _step = 1;
                                    let _enrollment_complete = false;
                                    let mut payment_confirmed = false;
                                    let mut pricing_result = None;
                                    let mut trip_from = "".to_string();
                                    let mut trip_to = "".to_string();
                                    
                                    for (tool_name, arguments) in &tool_calls {
                                        // Non-payment tools
                                        if !tool_name.contains("enroll") && !tool_name.contains("purchase") && !tool_name.contains("retrieve") {
                                            println!("→ Invoking: {} with args {}", tool_name, arguments);

                                            // Extract from/to from pricing tool arguments
                                            if tool_name == "get-ticket-price" {
                                                if let Some(from_val) = arguments.get("from").and_then(|v| v.as_str()) {
                                                    trip_from = from_val.to_string();
                                                }
                                                if let Some(to_val) = arguments.get("to").and_then(|v| v.as_str()) {
                                                    trip_to = to_val.to_string();
                                                }
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
                                                    println!("✓ Result: {}\n", result);
                                                    
                                                    // Store pricing result
                                                    if tool_name == "get-ticket-price" {
                                                        pricing_result = Some(result.clone());
                                                    }
                                                }
                                                Err(e) => {
                                                    println!("✗ Error: {}\n", e);
                                                }
                                            }
                                        }
                                    }
                                    
                                    // If we have pricing, present it and ask for confirmation
                                    if let Some(pricing) = pricing_result {
                                        if let Ok(parsed) = serde_json::from_str::<Value>(&pricing) {
                                            if let Some(price) = parsed.get("price") {
                                                println!("Agent A: Great! I found a flight from {} to {} for ${}.", trip_from, trip_to, price);
                                                println!("Agent A: This includes all taxes and fees.\n");
                                                
                                                // Ask user if they want to proceed
                                                if ask_confirmation_from_reader("Would you like to proceed with this booking?", &mut reader, &mut stdout)? {
                                                    // Get passenger details
                                                    print!("Please enter your full name: ");
                                                    stdout.flush()?;
                                                    let mut passenger_name = String::new();
                                                    reader.read_line(&mut passenger_name)?;
                                                    let passenger_name = passenger_name.trim().to_string();
                                                    
                                                    print!("Please enter your email address: ");
                                                    stdout.flush()?;
                                                    let mut passenger_email = String::new();
                                                    reader.read_line(&mut passenger_email)?;
                                                    let passenger_email = passenger_email.trim().to_string();
                                                    
                                                    // Ask about payment method
                                                    println!("\nAgent A: Great! Let's set up your payment.\n");
                                                    println!("How would you like to pay?");
                                                    println!("  1. Visa Credit Card");
                                                    println!("  2. Other payment method\n");
                                                    
                                                    print!("Choose payment method [1-2]: ");
                                                    stdout.flush()?;
                                                    
                                                    let mut payment_choice = String::new();
                                                    reader.read_line(&mut payment_choice)?;
                                                    
                                                    let payment_method = match payment_choice.trim() {
                                                        "1" => "Visa Credit Card",
                                                        "2" => {
                                                            println!("Agent A: Other payment methods are not yet supported. Please choose Visa.\n");
                                                            "Visa Credit Card"
                                                        }
                                                        _ => {
                                                            println!("Agent A: Invalid choice. Using Visa Credit Card.\n");
                                                            "Visa Credit Card"
                                                        }
                                                    };
                                                    
                                                    println!("Agent A: Perfect! I'll set up your {} for this transaction.\n", payment_method);
                                                    
                                                    // User confirmed, proceed directly with payment
                                                    println!("Agent A: To proceed with the booking, I'll need to set up payment.\n");
                                                    
                                                    // Enrollment step
                                                    show_step(2, 3, "Enrolling your payment card...");
                                                    
                                                    let mut enrollment_complete = false;
                                                    let mut enrollment_token_id = "token_789".to_string();
                                                    
                                                    // Check if card is already enrolled
                                                    let session_id = "sess_user_123".to_string();
                                                    let session_url = format!("{}/session/{}", 
                                                        payment_agent_url.unwrap_or("http://localhost:3002"), 
                                                        session_id);
                                                    
                                                    match client.get(&session_url).send().await {
                                                        Ok(response) => {
                                                            if let Ok(session_data) = response.json::<Value>().await {
                                                                if let Some(data) = session_data.get("data") {
                                                                    if let Some(token_count) = data.get("enrolledTokenCount").and_then(|c| c.as_u64()) {
                                                                        if token_count > 0 {
                                                                            println!("Agent A: I found an existing payment card in your account.\n");
                                                                            show_success("Your card is already enrolled with biometric authentication!");
                                                                            enrollment_complete = true;
                                                                            
                                                                            // Extract the first enrolled token ID
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
                                                        Err(_) => {
                                                            // Session check failed, proceed with enrollment
                                                        }
                                                    }
                                                    
                                                    // If not enrolled, ask user to enroll
                                                    if !enrollment_complete {
                                                        println!("Agent A: Let me securely add your card for this transaction.");
                                                        println!("Agent A: You'll authenticate using your device's biometric authentication (Face ID/Fingerprint).\n");
                                                        
                                                        if ask_confirmation_from_reader("Ready to add your card?", &mut reader, &mut stdout)? {
                                                            show_status("Adding your card...");
                                                            
                                                            let enroll_args = json!({
                                                                "sessionId": session_id,
                                                                "consumerId": "user_123",
                                                                "enrollmentReferenceId": "enroll_ref_456"
                                                            });
                                                            
                                                            println!("→ Invoking: enroll-card with args {}", enroll_args);

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
                                                                            // Try to get tokenId from data.tokenId (payment agent response format)
                                                                            let token_id = parsed
                                                                                .get("data")
                                                                                .and_then(|data| data.get("tokenId"))
                                                                                .or_else(|| parsed.get("tokenId"))
                                                                                .and_then(|t| t.as_str());
                                                                            
                                                                            if let Some(token_id) = token_id {
                                                                                enrollment_token_id = token_id.to_string();
                                                                            }
                                                                            show_success("Your card has been enrolled with biometric authentication!");
                                                                            enrollment_complete = true;
                                                                        } else {
                                                                            println!("✗ Enrollment failed: {}\n", result);
                                                                        }
                                                                    } else {
                                                                        println!("✓ Result: {}\n", result);
                                                                        enrollment_complete = true;
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    println!("✗ Error: {}\n", e);
                                                                }
                                                            }
                                                        } else {
                                                            println!("Agent A: Card enrollment cancelled. Unable to proceed with payment.\n");
                                                            continue;
                                                        }
                                                    }
                                                    
                                                    // Payment confirmation step
                                                    if enrollment_complete {
                                                        show_step(3, 3, "Confirming payment...");
                                                        
                                                        println!("Agent A: Your card is ready. Shall I proceed with the payment?\n");
                                                        
                                                        if ask_confirmation_from_reader("Proceed with payment?", &mut reader, &mut stdout)? {
                                                            show_status("Processing payment...");
                                                            show_status("You'll be asked to authenticate with biometric on your device...");
                                                            
                                                            // Execute purchase
                                                            let purchase_args = json!({
                                                                "sessionId": "sess_user_123",
                                                                "consumerId": "user_123",
                                                                "tokenId": enrollment_token_id,
                                                                "amount": price.to_string(),
                                                                "merchant": "ZeroProof Travel"
                                                            });
                                                            
                                                            println!("→ Invoking: initiate-purchase-instruction with args {}", purchase_args);

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
                                                                    println!("✓ Result: {}\n", result);
                                                                    
                                                                    // Extract instructionId from purchase result
                                                                    if let Ok(purchase_response) = serde_json::from_str::<Value>(&result) {
                                                                        // Try to get instructionId from data.instructionId (payment agent response format)
                                                                        let instruction_id = purchase_response
                                                                            .get("data")
                                                                            .and_then(|data| data.get("instructionId"))
                                                                            .or_else(|| purchase_response.get("instructionId"))
                                                                            .and_then(|id| id.as_str());
                                                                        
                                                                        if let Some(instruction_id) = instruction_id {
                                                                            // Execute credential retrieval with actual instructionId
                                                                            let retrieve_args = json!({
                                                                                "sessionId": "sess_user_123",
                                                                                "consumerId": "user_123",
                                                                                "tokenId": enrollment_token_id,
                                                                                "instructionId": instruction_id,
                                                                                "transactionReferenceId": "txn_202"
                                                                            });
                                                                            
                                                                            println!("→ Invoking: retrieve-payment-credentials with args {}", retrieve_args);

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
                                                                                Ok(result) => {
                                                                                    println!("✓ Result: {}\n", result);
                                                                                    payment_confirmed = true;
                                                                                }
                                                                                Err(e) => {
                                                                                    println!("✗ Error: {}\n", e);
                                                                                }
                                                                            }
                                                                        } else {
                                                                            println!("✗ Error: Could not extract instructionId from purchase response\n");
                                                                        }
                                                                    } else {
                                                                        println!("✗ Error: Could not parse purchase response\n");
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    println!("✗ Error: {}\n", e);
                                                                }
                                                            }
                                                            
                                                            if payment_confirmed {
                                                                show_success("Payment confirmed! Now I am going to complete your booking!");
                                                                
                                                                // Now call book-flight with passenger details
                                                                show_step(3, 3, "Completing your flight booking...");
                                                                
                                                                let book_args = json!({
                                                                    "from": trip_from,
                                                                    "to": trip_to,
                                                                    "passenger_name": passenger_name,
                                                                    "passenger_email": passenger_email
                                                                });
                                                                
                                                                println!("→ Invoking: book-flight with args {}", book_args);

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
                                                                        println!("✓ Result: {}\n", result);
                                                                        if let Ok(booking) = serde_json::from_str::<Value>(&result) {
                                                                            if let Some(conf_code) = booking.get("confirmation_code").and_then(|c| c.as_str()) {
                                                                                show_success("Flight booking confirmed!");
                                                                                println!("Agent A: Your flight booking from {} to {} has been confirmed.\n", trip_from, trip_to);
                                                                                println!("Agent A: Confirmation code: {}\n", conf_code);
                                                                                println!("Agent A: You'll receive a confirmation email shortly with your flight details and receipt.\n");
                                                                            }
                                                                        }
                                                                    }
                                                                    Err(e) => {
                                                                        println!("✗ Error booking flight: {}\n", e);
                                                                    }
                                                                }
                                                            }
                                                        } else {
                                                            println!("Agent A: Payment cancelled. Your booking has been cancelled.\n");
                                                        }
                                                    }
                                                } else {
                                                    println!("Agent A: Okay, I've cancelled the booking. Let me know if you'd like to try different dates or destinations.\n");
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                    
                                } else {
                                    // Non-payment tool flow (existing behavior)
                                    for (tool_name, arguments) in tool_calls {
                                        println!("→ Invoking: {} with args {}", tool_name, arguments);

                                        match call_server_tool(
                                            &client,
                                            &config.server_url,
                                            &agent_b_url,
                                            payment_agent_url,
                                            &tool_name,
                                            arguments,
                                        )
                                        .await
                                        {
                                            Ok(result) => {
                                                println!("✓ Result: {}\n", result);
                                            }
                                            Err(e) => {
                                                println!("✗ Error: {}\n", e);
                                            }
                                        }
                                    }

                                    // Extract user message from Claude response
                                    if let Ok(parsed) = serde_json::from_str::<Value>(&claude_response) {
                                        if let Some(msg) = parsed.get("user_message").and_then(|m| m.as_str()) {
                                            println!("Agent A: {}\n", msg);
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            // Parse failed, show as conversational response
                            println!("Agent A: {}\n", claude_response);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("✗ Claude API error: {}\n", e);
                }
            }
    }

    Ok(())
}
