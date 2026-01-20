/// Tool definition fetching logic
/// Handles fetching and caching of tool definitions from Agent A, Agent B, and Payment Agent servers

use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use tokio::sync::Mutex;

// Thread-safe cache for tool definitions
// Tools are fetched once at first request and then reused for all subsequent requests
// This prevents the repeated GET /tools calls to payment agent on every chat message
lazy_static::lazy_static! {
    static ref TOOLS_CACHE: Mutex<Option<Value>> = Mutex::new(None);
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

/// Fetch and merge tool definitions from all servers with caching
/// Tools are fetched only once on first request, then cached and reused
/// This prevents repeated GET /tools calls to payment agent on every chat message
pub async fn fetch_all_tools(
    client: &reqwest::Client,
    agent_a_url: &str,
    agent_b_url: &str,
    payment_agent_url: Option<&str>,
) -> Result<Value> {
    // Check if tools are already cached
    let mut cache = TOOLS_CACHE.lock().await;
    
    if let Some(cached_tools) = cache.as_ref() {
        println!("[TOOLS CACHE] ✓ Using cached tool definitions (avoids repeated /tools calls)");
        return Ok(cached_tools.clone());
    }
    
    println!("[TOOLS CACHE] Cache miss - fetching tool definitions from all servers...");
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
                    println!("[TOOLS CACHE] ✓ Fetched payment agent tools");
                }
            }
            Err(e) => {
                eprintln!("Warning: Could not fetch Payment Agent tools: {}", e);
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
    
    let result = json!({ "tools": all_tools });
    
    // Store in cache for future requests
    *cache = Some(result.clone());
    println!("[TOOLS CACHE] ✓ Cached {} tools for future requests", all_tools.len());
    
    Ok(result)
}

/// Parse Claude's tool recommendations from JSON response
pub fn parse_tool_calls(claude_response: &str) -> Result<Vec<(String, Value)>> {
    let json_start = claude_response.find('{');
    let json_end = claude_response.rfind('}');
    
    println!("[PARSER] Looking for JSON in response (length: {})", claude_response.len());
    println!("[PARSER] First {{ at: {:?}, Last }} at: {:?}", json_start, json_end);

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &claude_response[start..=end];
        println!("[PARSER] Extracted JSON (length: {}): {}", json_str.len(), 
                 &json_str[..json_str.len().min(200)]);
        println!("[PARSER] Last 100 chars: {}", 
                 &json_str[json_str.len().saturating_sub(100)..]);
        
        match serde_json::from_str::<Value>(json_str) {
            Ok(parsed) => {
                println!("[PARSER] ✓ JSON parsed successfully");
                println!("[PARSER] Root keys: {:?}", parsed.as_object().map(|o| o.keys().collect::<Vec<_>>()));
                
                let mut tools = Vec::new();
                if let Some(tool_calls) = parsed.get("tool_calls").and_then(|t| t.as_array()) {
                    println!("[PARSER] ✓ Found tool_calls array with {} items", tool_calls.len());
                    for (i, call) in tool_calls.iter().enumerate() {
                        if let (Some(name), Some(args)) = (
                            call.get("name").and_then(|n| n.as_str()),
                            call.get("arguments"),
                        ) {
                            println!("[PARSER]   Tool {}: name={}", i, name);
                            tools.push((name.to_string(), args.clone()));
                        }
                    }
                } else {
                    println!("[PARSER] ✗ tool_calls field not found or not an array");
                    if let Some(tc) = parsed.get("tool_calls") {
                        println!("[PARSER] tool_calls value: {:?}", tc);
                    }
                }
                Ok(tools)
            }
            Err(e) => {
                println!("[PARSER] ✗ JSON parse error: {}", e);
                println!("[PARSER] Full extracted JSON for debugging:");
                println!("{}", json_str);
                Err(anyhow!("JSON parse error: {}", e))
            }
        }
    } else {
        println!("[PARSER] ✗ Could not find {{ or }}");
        Err(anyhow!("Could not parse tool calls from Claude response"))
    }
}
