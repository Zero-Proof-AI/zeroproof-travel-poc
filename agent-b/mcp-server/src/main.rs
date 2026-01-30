/// Agent B MCP Server - Pricing & Booking Service
///
/// Exposes pricing and booking operations as MCP tools over HTTP API
/// - POST /tools/get-ticket-price
/// - POST /tools/book-flight
/// - GET /tools - List all tools

use anyhow::Result;
use axum::{
    extract::Json,
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::cors::CorsLayer;

mod validate;

use pricing_core::pricing;
use pricing_core::booking;
use validate::verify_payment_proof;

/// Pricing Tool Request
#[derive(Debug, Deserialize)]
struct PriceRequest {
    from: String,
    to: String,
    vip: Option<bool>,
}

/// Pricing Tool Response
#[derive(Debug, Serialize)]
struct PriceResponse {
    price: f64,
    from: String,
    to: String,
    vip: bool,
    currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<serde_json::Value>,
}

/// Booking Tool Request
#[derive(Debug, Deserialize)]
struct BookRequest {
    from: String,
    to: String,
    passenger_name: String,
    passenger_email: String,
    #[serde(default)]
    session_id: Option<String>,
}

/// Booking Tool Response
#[derive(Debug, Serialize)]
struct BookResponse {
    booking_id: String,
    status: String,
    confirmation_code: String,
    from: String,
    to: String,
    passenger_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<serde_json::Value>,
}

/// Tool Definition
#[derive(Debug, Serialize)]
struct ToolDefinition {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

/// Tools List Response
#[derive(Debug, Serialize)]
struct ToolsResponse {
    tools: Vec<ToolDefinition>,
}

/// Standard Tool Response
#[derive(Debug, Serialize)]
struct ToolResponse<T: Serialize> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T: Serialize> ToolResponse<T> {
    fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
}

fn tool_error(error: String) -> ToolResponse<()> {
    ToolResponse {
        success: false,
        data: None,
        error: Some(error),
    }
}

/// Health check endpoint
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "healthy"}))
}

/// List all available tools
async fn list_tools() -> Json<ToolsResponse> {
    tracing::info!("[LIST TOOLS] Received request to list available tools");
    Json(ToolsResponse {
        tools: vec![
            ToolDefinition {
                name: "get-ticket-price".to_string(),
                description: "Get flight ticket pricing based on route and passenger tier".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "from": {
                            "type": "string",
                            "description": "Departure city code (e.g., NYC)"
                        },
                        "to": {
                            "type": "string",
                            "description": "Destination city code (e.g., LON)"
                        },
                        "vip": {
                            "type": "boolean",
                            "description": "Whether passenger is VIP (optional, default false)"
                        }
                    },
                    "required": ["from", "to"]
                }),
            },
            ToolDefinition {
                name: "book-flight".to_string(),
                description: "Book a flight and generate confirmation".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "from": {
                            "type": "string",
                            "description": "Departure city code"
                        },
                        "to": {
                            "type": "string",
                            "description": "Destination city code"
                        },
                        "passenger_name": {
                            "type": "string",
                            "description": "Full name of passenger"
                        },
                        "passenger_email": {
                            "type": "string",
                            "description": "Email address of passenger"
                        }
                    },
                    "required": ["from", "to", "passenger_name", "passenger_email"]
                }),
            },
        ],
    })
}

/// Get ticket pricing
async fn get_ticket_price(
    Json(req): Json<PriceRequest>,
) -> Result<Json<ToolResponse<PriceResponse>>, (StatusCode, Json<ToolResponse<()>>)> {
    tracing::info!("[GET-TICKET-PRICE] Tool call received: from={}, to={}, vip={:?}", req.from, req.to, req.vip);
    
    // Validate input with specific error messages
    if req.from.is_empty() && req.to.is_empty() {
        tracing::warn!("[GET-TICKET-PRICE] Validation failed: both departure and destination are missing");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(
                "Missing required fields: 'from' (departure city) and 'to' (destination city) are both required".to_string(),
            )),
        ));
    }
    
    if req.from.is_empty() {
        tracing::warn!("[GET-TICKET-PRICE] Validation failed: departure city is missing");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(
                "Missing required field: 'from' (departure city code, e.g., NYC, LON, LAX)".to_string(),
            )),
        ));
    }
    
    if req.to.is_empty() {
        tracing::warn!("[GET-TICKET-PRICE] Validation failed: destination city is missing");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(
                "Missing required field: 'to' (destination city code, e.g., NYC, LON, LAX)".to_string(),
            )),
        ));
    }

    // Use pricing-core to calculate price
    let core_req = pricing::Request {
        from: req.from.clone(),
        to: req.to.clone(),
        vip: req.vip.unwrap_or(false),
    };

    let core_resp = pricing::handle(core_req);
    
    tracing::info!("[GET-TICKET-PRICE] Successfully calculated price: ${} (vip={})", core_resp.price, req.vip.unwrap_or(false));

    Ok(Json(ToolResponse::ok(PriceResponse {
        price: core_resp.price,
        from: req.from,
        to: req.to,
        vip: req.vip.unwrap_or(false),
        currency: "USD".to_string(),
        proof: None,  // Pricing is a deterministic calculation, no proof generated
    })))
}

/// Book a flight
async fn book_flight(
    Json(req): Json<BookRequest>,
) -> Result<Json<ToolResponse<BookResponse>>, (StatusCode, Json<ToolResponse<()>>)> {
    tracing::info!("[BOOK-FLIGHT] Tool call received: from={}, to={}, passenger={}, email={}", req.from, req.to, req.passenger_name, req.passenger_email);
    
    // Get zkfetch URL from environment
    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
        .unwrap_or_else(|_| "http://localhost:8003".to_string());

    // Validate input with specific error messages
    let mut missing_fields = Vec::new();
    
    if req.from.is_empty() {
        missing_fields.push("'from' (departure city code, e.g., NYC)");
    }
    if req.to.is_empty() {
        missing_fields.push("'to' (destination city code, e.g., LON)");
    }
    if req.passenger_name.is_empty() {
        missing_fields.push("'passenger_name' (full name of passenger)");
    }
    if req.passenger_email.is_empty() {
        missing_fields.push("'passenger_email' (email address)");
    }
    
    if !missing_fields.is_empty() {
        let error_msg = format!(
            "Missing required field(s): {}",
            missing_fields.join(", ")
        );
        tracing::warn!("[BOOK-FLIGHT] Validation failed: {}", error_msg);
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(error_msg)),
        ));
    }

    // Session_id must be provided by agent-a for proof tracking across the workflow
    let session_id = req.session_id.clone()
        .ok_or_else(|| {
            tracing::warn!("[BOOK-FLIGHT] Missing required field: session_id");
            (
                StatusCode::BAD_REQUEST,
                Json(tool_error("Missing required field: 'session_id' (must be provided by orchestrator)".to_string())),
            )
        })?;

    // HARD BLOCK: Verify payment proof from attestation service
    // This ensures Agent-A actually completed the payment before we book the flight
    let attestation_url = std::env::var("ATTESTER_URL")
        .unwrap_or_else(|_| "http://localhost:8002".to_string());
    
    if let Err(payment_error) = verify_payment_proof(&session_id, &attestation_url).await {
        tracing::error!("[BOOK-FLIGHT] Payment verification FAILED: {}", payment_error);
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(tool_error(format!(
                "Payment verification failed - cannot book flight: {}",
                payment_error
            ))),
        ));
    }
    tracing::info!("[BOOK-FLIGHT] ✓ Payment proof verified - proceeding with booking");

    // Delegate to pricing-core library handle_async for business logic
    let core_req = booking::Request {
        from: req.from.clone(),
        to: req.to.clone(),
        passenger_name: req.passenger_name.clone(),
        passenger_email: req.passenger_email.clone(),
    };

    let (response, proof) = booking::handle_async(core_req, zkfetch_url, &session_id).await;

    tracing::info!("[BOOK-FLIGHT] result: booking_id={}, confirmation_code={}, status={}", response.booking_id, response.confirmation_code, response.status);

    // NOTE: Proof is automatically submitted by proxy_fetch's submit_proof_async()
    // when the attestation_config is enabled in the ProxyConfig
    
    Ok(Json(ToolResponse::ok(BookResponse {
        booking_id: response.booking_id,
        status: response.status,
        confirmation_code: response.confirmation_code,
        from: req.from,
        to: req.to,
        passenger_name: req.passenger_name,
        proof,
    })))
}

#[derive(Debug, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct McpResponse {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpError>,
}

#[derive(Debug, Serialize)]
struct McpError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// MCP Initialize Response
#[derive(Debug, Serialize)]
struct InitializeResult {
    protocol_version: String,
    capabilities: serde_json::Value,
    server_info: serde_json::Value,
}

/// MCP Tools List Response
#[derive(Debug, Serialize)]
struct ToolsListResult {
    tools: Vec<McpTool>,
}

#[derive(Debug, Serialize)]
struct McpTool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

/// MCP Tool Call Response
#[derive(Debug, Serialize)]
struct ToolCallResult {
    content: Vec<serde_json::Value>,
    #[serde(rename = "isError")]
    is_error: bool,
}

/// Handle MCP protocol requests
async fn handle_mcp(
    Json(req): Json<McpRequest>,
) -> Result<Json<McpResponse>, (StatusCode, Json<McpResponse>)> {
    tracing::info!("[MCP] Received request: method={}, id={:?}", req.method, req.id);

    match req.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                capabilities: json!({
                    "tools": {}
                }),
                server_info: json!({
                    "name": "agent-b-mcp-server",
                    "version": "1.0.0"
                }),
            };

            Ok(Json(McpResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }))
        }

        "tools/list" => {
            let tools = vec![
                McpTool {
                    name: "get-ticket-price".to_string(),
                    description: "Get flight ticket pricing based on route and passenger tier".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "from": {
                                "type": "string",
                                "description": "Departure city code (e.g., NYC)"
                            },
                            "to": {
                                "type": "string",
                                "description": "Destination city code (e.g., LON)"
                            },
                            "vip": {
                                "type": "boolean",
                                "description": "Whether passenger is VIP (optional, default false)"
                            }
                        },
                        "required": ["from", "to"]
                    }),
                },
                McpTool {
                    name: "book-flight".to_string(),
                    description: "Book a flight and generate confirmation".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "from": {
                                "type": "string",
                                "description": "Departure city code"
                            },
                            "to": {
                                "type": "string",
                                "description": "Destination city code"
                            },
                            "passenger_name": {
                                "type": "string",
                                "description": "Full name of passenger"
                            },
                            "passenger_email": {
                                "type": "string",
                                "description": "Email address of passenger"
                            }
                        },
                        "required": ["from", "to", "passenger_name", "passenger_email"]
                    }),
                },
            ];

            let result = ToolsListResult { tools };

            Ok(Json(McpResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }))
        }

        "tools/call" => {
            let params = req.params.ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id.clone(),
                        result: None,
                        error: Some(McpError {
                            code: -32602,
                            message: "Invalid params".to_string(),
                            data: None,
                        }),
                    }),
                )
            })?;

            let tool_name = params.get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id.clone(),
                            result: None,
                            error: Some(McpError {
                                code: -32602,
                                message: "Tool name required".to_string(),
                                data: None,
                            }),
                        }),
                    )
                })?;

            let tool_args = params.get("arguments")
                .and_then(|a| a.as_object())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id.clone(),
                            result: None,
                            error: Some(McpError {
                                code: -32602,
                                message: "Tool arguments required".to_string(),
                                data: None,
                            }),
                        }),
                    )
                })?;

            match tool_name {
                "get-ticket-price" => {
                    let from = tool_args.get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let to = tool_args.get("to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let vip = tool_args.get("vip")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    // Validate required fields (same as existing get_ticket_price handler)
                    if from.is_empty() && to.is_empty() {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id.clone(),
                                result: None,
                                error: Some(McpError {
                                    code: -32602,
                                    message: "Missing required fields: 'from' (departure city) and 'to' (destination city) are both required".to_string(),
                                    data: None,
                                }),
                            }),
                        ));
                    }

                    if from.is_empty() {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id.clone(),
                                result: None,
                                error: Some(McpError {
                                    code: -32602,
                                    message: "Missing required field: 'from' (departure city code, e.g., NYC, LON, LAX)".to_string(),
                                    data: None,
                                }),
                            }),
                        ));
                    }

                    if to.is_empty() {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id.clone(),
                                result: None,
                                error: Some(McpError {
                                    code: -32602,
                                    message: "Missing required field: 'to' (destination city code, e.g., NYC, LON, LAX)".to_string(),
                                    data: None,
                                }),
                            }),
                        ));
                    }

                    // Use the existing pricing::handle from pricing-core (same as existing handler)
                    let core_req = pricing::Request {
                        from: from.to_string(),
                        to: to.to_string(),
                        vip,
                    };

                    let core_resp = pricing::handle(core_req);

                    let content = vec![json!({
                        "type": "text",
                        "text": serde_json::to_string(&PriceResponse {
                            price: core_resp.price,
                            from: from.to_string(),
                            to: to.to_string(),
                            vip,
                            currency: "USD".to_string(),
                            proof: None,
                        }).unwrap()
                    })];

                    let result = ToolCallResult {
                        content,
                        is_error: false,
                    };

                    Ok(Json(McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                    }))
                }

                "book-flight" => {
                    let from = tool_args.get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let to = tool_args.get("to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let passenger_name = tool_args.get("passenger_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let passenger_email = tool_args.get("passenger_email")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    // For MCP calls, generate a session_id (no validation/payment checks)
                    let session_id = format!("mcp-session-{}", req.id.as_ref()
                        .and_then(|id| id.as_str())
                        .unwrap_or("unknown"));

                    // Get zkfetch URL from environment
                    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
                        .unwrap_or_else(|_| "http://localhost:8003".to_string());

                    // Call handle_async directly (no validation/payment verification for MCP)
                    let core_req = booking::Request {
                        from: from.to_string(),
                        to: to.to_string(),
                        passenger_name: passenger_name.to_string(),
                        passenger_email: passenger_email.to_string(),
                    };

                    let (response, proof) = booking::handle_async(core_req, zkfetch_url, &session_id).await;

                    let content = vec![json!({
                        "type": "text",
                        "text": serde_json::to_string(&BookResponse {
                            booking_id: response.booking_id,
                            status: response.status,
                            confirmation_code: response.confirmation_code,
                            from: from.to_string(),
                            to: to.to_string(),
                            passenger_name: passenger_name.to_string(),
                            proof,
                        }).unwrap()
                    })];

                    let result = ToolCallResult {
                        content,
                        is_error: false,
                    };

                    Ok(Json(McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                    }))
                }

                _ => {
                    Err((
                        StatusCode::NOT_FOUND,
                        Json(McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: req.id,
                            result: None,
                            error: Some(McpError {
                                code: -32601,
                                message: format!("Tool '{}' not found", tool_name),
                                data: None,
                            }),
                        }),
                    ))
                }
            }
        }

        _ => {
            Err((
                StatusCode::METHOD_NOT_ALLOWED,
                Json(McpResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(McpError {
                        code: -32601,
                        message: format!("Method '{}' not found", req.method),
                        data: None,
                    }),
                }),
            ))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║          Agent B - MCP Server (Pricing & Booking)          ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/tools", get(list_tools))
        .route("/tools/get-ticket-price", post(get_ticket_price))
        .route("/tools/book-flight", post(book_flight))
        .route("/mcp", post(handle_mcp))
        .layer(CorsLayer::permissive());

    // Get port from environment variable or use default
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8001".to_string())
        .parse::<u16>()?;
    let addr = format!("0.0.0.0:{}", port);

    // Bind and serve
    let listener = tokio::net::TcpListener::bind(&addr)
        .await?;

    let attester_url = std::env::var("ATTESTER_URL")
        .unwrap_or_else(|_| "http://localhost:8000".to_string());

    // Print zkfetch endpoint configuration
    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
        .unwrap_or_else(|_| "http://localhost:8003".to_string());
    
    println!("[INIT] Server configuration:");
    println!("🔐 zkfetch Endpoint: {}/zkfetch", zkfetch_url);
    println!("📍 Attestation Service: {}\n", attester_url);
    
    println!("✓ Agent B MCP Server running on http://0.0.0.0:{}", port);
    println!("  GET  /tools                     — List all tools");
    println!("  POST /tools/get-ticket-price    — Get flight pricing");
    println!("  POST /tools/book-flight         — Book a flight");
    println!("  POST /mcp                       — MCP protocol endpoint\n");

    
    
    

    axum::serve(listener, app).await?;

    Ok(())
}
