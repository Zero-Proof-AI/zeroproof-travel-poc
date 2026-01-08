/// Agent A MCP Server - JSON-RPC (stdio) + HTTP API
///
/// Dual-protocol server:
/// 1. JSON-RPC over stdin/stdout (for direct MCP protocol)
/// 2. HTTP endpoints (for remote/network access)
///
/// Run with HTTP: AGENT_A_MODE=http ./agent-a-mcp
/// Run with MCP:  ./agent-a-mcp (default)

use anyhow::{Result, anyhow};
use axum::{
    extract::Json,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use agent_a_mcp::{
    PricingInput,
    verify_on_chain, get_ticket_price, format_zk_input, request_attestation,
};

/// Agent A Server - holds tool implementations
#[derive(Clone)]
struct AgentAMcp {
    agent_b_url: Arc<String>,
    attester_url: Arc<String>,
    zeroproof_addr: Arc<String>,
    rpc_url: Arc<String>,
}

impl AgentAMcp {
    fn new() -> Self {
        Self {
            agent_b_url: Arc::new(
                std::env::var("AGENT_B_MCP_URL")
                    .unwrap_or_else(|_| "http://localhost:8001".to_string()),
            ),
            attester_url: Arc::new(
                std::env::var("ATTESTER_URL")
                    .unwrap_or_else(|_| "http://localhost:8000".to_string()),
            ),
            zeroproof_addr: Arc::new(
                std::env::var("ZEROPROOF_ADDRESS")
                    .unwrap_or_else(|_| "0x9C33252D29B41Fe2706704a8Ca99E8731B58af41".to_string()),
            ),
            rpc_url: Arc::new(
                std::env::var("RPC_URL")
                    .unwrap_or_else(|_| "https://sepolia.infura.io/v3/abc123".to_string()),
            ),
        }
    }

    /// List all available tools
    fn list_tools(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "get_ticket_price",
                    "description": "Get flight ticket pricing from Agent B",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "from": {"type": "string"},
                            "to": {"type": "string"},
                            "vip": {"type": "boolean"}
                        }
                    }
                },
                {
                    "name": "format_zk_input",
                    "description": "Format input for zkVM computation",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "endpoint": {"type": "string"},
                            "input": {"type": "object"}
                        }
                    }
                },
                {
                    "name": "request_attestation",
                    "description": "Request ZK proof from attester service",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "program_id": {"type": "string"},
                            "input_hex": {"type": "string"},
                            "claimed_output": {"type": "string"}
                        }
                    }
                },
                {
                    "name": "verify_on_chain",
                    "description": "Verify ZK proof on Sepolia blockchain",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "proof": {"type": "string"},
                            "public_values": {"type": "string"},
                            "vk_hash": {"type": "string"}
                        }
                    }
                }
            ]
        })
    }

    /// Call a tool and return result
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        match name {
            "get_ticket_price" => {
                let from = arguments
                    .get("from")
                    .and_then(|v| v.as_str())
                    .unwrap_or("NYC");
                let to = arguments
                    .get("to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("LON");
                let vip = arguments
                    .get("vip")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let input = PricingInput {
                    from: from.to_string(),
                    to: to.to_string(),
                    vip,
                };

                match get_ticket_price(&self.agent_b_url, &input).await {
                    Ok(response) => Ok(json!({
                        "price": response.price,
                        "program_id": response.program_id,
                        "elf_hash": response.elf_hash
                    })),
                    Err(e) => Err(anyhow!("Agent B call failed: {}", e)),
                }
            }

            "format_zk_input" => {
                let endpoint = arguments
                    .get("endpoint")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let input = arguments.get("input").cloned().unwrap_or(json!({}));

                match format_zk_input(&self.agent_b_url, endpoint, &input).await {
                    Ok(result) => Ok(json!({
                        "input_hex": result.input_bytes,
                        "length": result.input_array.len()
                    })),
                    Err(e) => Err(anyhow!("Format ZK input failed: {}", e)),
                }
            }

            "request_attestation" => {
                let program_id = arguments
                    .get("program_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let input_hex = arguments
                    .get("input_hex")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0x");

                let input_bytes = hex::decode(input_hex.strip_prefix("0x").unwrap_or(input_hex))
                    .map_err(|e| anyhow!("Invalid hex: {}", e))?;
                let claimed_output = arguments.get("claimed_output").cloned();

                match request_attestation(
                    &self.attester_url,
                    program_id,
                    input_bytes,
                    claimed_output,
                    true,
                )
                .await
                {
                    Ok(response) => Ok(json!({
                        "verified_output": response.verified_output,
                        "vk_hash": response.vk_hash
                    })),
                    Err(e) => Err(anyhow!("Attestation request failed: {}", e)),
                }
            }

            "verify_on_chain" => {
                let proof = arguments
                    .get("proof")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0x");
                let public_values = arguments
                    .get("public_values")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0x");
                let vk_hash = arguments
                    .get("vk_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0x");

                match verify_on_chain(&self.zeroproof_addr, &self.rpc_url, proof, public_values, vk_hash).await {
                    Ok(verified) => Ok(json!({
                        "verified": verified,
                        "message": if verified {
                            "✓ Proof verified on-chain"
                        } else {
                            "✗ Proof verification failed"
                        }
                    })),
                    Err(e) => Err(anyhow!("On-chain verification error: {}", e)),
                }
            }

            _ => Err(anyhow!("Unknown tool: {}", name)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if running in HTTP mode or JSON-RPC mode
    let mode = std::env::var("AGENT_A_MODE").unwrap_or_else(|_| "http".to_string());
    
    match mode.as_str() {
        "jsonrpc" => run_jsonrpc_server().await,
        "http" | _ => start_http_server().await,
    }
}

async fn run_jsonrpc_server() -> Result<()> {
    let server = AgentAMcp::new();
    let stdin = io::stdin();
    let mut reader = stdin.lock().lines();

    // Read JSON-RPC messages from stdin
    while let Some(Ok(line)) = reader.next() {
        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON-RPC request
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Parse error: {}", e);
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(json!(null));
        let method = match request.get("method").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => continue,
        };

        let response = match method {
            "initialize" => {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11",
                        "capabilities": {"tools": {}},
                        "serverInfo": {
                            "name": "Agent A",
                            "version": "0.1.0"
                        }
                    }
                })
            }

            "tools/list" => {
                let tools = server.list_tools();
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": tools
                })
            }

            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                match server.call_tool(tool_name, arguments).await {
                    Ok(result) => {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{
                                    "type": "text",
                                    "text": result.to_string()
                                }]
                            }
                        })
                    }
                    Err(e) => {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32603,
                                "message": e.to_string()
                            }
                        })
                    }
                }
            }

            _ => {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {}", method)
                    }
                })
            }
        };

        // Send response
        println!("{}", response.to_string());
    }

    Ok(())
}

/// HTTP Response wrapper
#[derive(Debug, Serialize)]
struct HttpResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T> HttpResponse<T> {
    fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn err(error: impl std::fmt::Display) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error.to_string()),
        }
    }
}

/// HTTP request types
#[derive(Debug, Deserialize)]
struct CallAgentBRequest {
    from: String,
    to: String,
    vip: bool,
}

#[derive(Debug, Deserialize)]
struct FormatZkInputRequest {
    endpoint: String,
    input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RequestAttestationRequest {
    program_id: String,
    input_hex: String,
    #[serde(default)]
    claimed_output: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerifyOnChainRequest {
    proof: String,
    public_values: String,
    vk_hash: String,
}

// HTTP Handlers
async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "Agent A MCP Server",
        "protocols": ["http", "jsonrpc-stdio"],
        "version": "0.1.0"
    }))
}

async fn list_tools_http(
) -> Json<serde_json::Value> {
    let server = AgentAMcp::new();
    Json(server.list_tools())
}

async fn http_get_ticket_price(
    Json(req): Json<CallAgentBRequest>,
) -> impl IntoResponse {
    let server = AgentAMcp::new();
    let input = PricingInput {
        from: req.from,
        to: req.to,
        vip: req.vip,
    };

    match get_ticket_price(&server.agent_b_url, &input).await {
        Ok(response) => {
            (
                StatusCode::OK,
                Json(HttpResponse::ok(json!({
                    "price": response.price,
                    "program_id": response.program_id,
                    "elf_hash": response.elf_hash
                }))),
            )
                .into_response()
        }
        Err(e) => {
            let error_response: HttpResponse<Value> = HttpResponse::err(e.to_string());
            (
                StatusCode::BAD_REQUEST,
                Json(error_response),
            )
                .into_response()
        }
    }
}

async fn http_format_zk_input(
    Json(req): Json<FormatZkInputRequest>,
) -> impl IntoResponse {
    let server = AgentAMcp::new();

    match format_zk_input(&server.agent_b_url, &req.endpoint, &req.input).await {
        Ok(result) => {
            (
                StatusCode::OK,
                Json(HttpResponse::ok(json!({
                    "input_hex": result.input_bytes,
                    "length": result.input_array.len()
                }))),
            )
                .into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_REQUEST,
                Json(HttpResponse::<()>::err(e.to_string())),
            )
                .into_response()
        }
    }
}

async fn http_request_attestation(
    Json(req): Json<RequestAttestationRequest>,
) -> impl IntoResponse {
    let server = AgentAMcp::new();
    
    let input_bytes = match hex::decode(req.input_hex.strip_prefix("0x").unwrap_or(&req.input_hex))
    {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HttpResponse::<()>::err(format!("Invalid hex: {}", e))),
            )
                .into_response();
        }
    };

    match request_attestation(
        &server.attester_url,
        &req.program_id,
        input_bytes,
        req.claimed_output.as_deref().map(|s| serde_json::json!(s)),
        true,
    )
    .await
    {
        Ok(response) => {
            (
                StatusCode::OK,
                Json(HttpResponse::ok(json!({
                    "verified_output": response.verified_output,
                    "vk_hash": response.vk_hash
                }))),
            )
                .into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_REQUEST,
                Json(HttpResponse::<()>::err(e.to_string())),
            )
                .into_response()
        }
    }
}

async fn http_verify_on_chain(
    Json(req): Json<VerifyOnChainRequest>,
) -> impl IntoResponse {
    let server = AgentAMcp::new();

    match verify_on_chain(
        &server.zeroproof_addr,
        &server.rpc_url,
        &req.proof,
        &req.public_values,
        &req.vk_hash,
    )
    .await
    {
        Ok(verified) => {
            (
                StatusCode::OK,
                Json(HttpResponse::ok(json!({
                    "verified": verified,
                    "message": if verified {
                        "✓ Proof verified on-chain"
                    } else {
                        "✗ Proof verification failed"
                    }
                }))),
            )
                .into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_REQUEST,
                Json(HttpResponse::<()>::err(e.to_string())),
            )
                .into_response()
        }
    }
}

/// Start HTTP server
async fn start_http_server() -> Result<()> {
    let port = std::env::var("AGENT_A_SERVER_PORT")
        .unwrap_or_else(|_| "3001".to_string())
        .parse::<u16>()
        .unwrap_or(3001);

    let app = Router::new()
        .route("/health", get(health))
        .route("/tools", get(list_tools_http))
        .route("/tools/get_ticket_price", post(http_get_ticket_price))
        .route("/tools/format_zk_input", post(http_format_zk_input))
        .route("/tools/request_attestation", post(http_request_attestation))
        .route("/tools/verify_on_chain", post(http_verify_on_chain))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║           Agent A - HTTP Server                            ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");
    println!("✓ Server listening on http://0.0.0.0:{}\n", port);
    println!("Endpoints:");
    println!("  GET    http://localhost:{}/health", port);
    println!("  GET    http://localhost:{}/tools", port);
    println!("  POST   http://localhost:{}/tools/get_ticket_price", port);
    println!("  POST   http://localhost:{}/tools/format_zk_input", port);
    println!("  POST   http://localhost:{}/tools/request_attestation", port);
    println!("  POST   http://localhost:{}/tools/verify_on_chain\n", port);

    axum::serve(listener, app).await?;

    Ok(())
}
