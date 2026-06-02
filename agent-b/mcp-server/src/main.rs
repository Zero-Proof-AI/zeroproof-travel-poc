/// Agent B MCP Server - Pricing & Booking Service
///
/// Exposes pricing and booking operations as MCP tools over HTTP API
/// - POST /tools/get-ticket-price
/// - POST /tools/book-flight
/// - GET /tools - List all tools

use anyhow::Result;
use axum::{
    extract::{Json, State},
    http::StatusCode,
    routing::{get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

mod validate;
mod farm;

use pricing_core::pricing;
use pricing_core::booking;
use validate::verify_payment_proof;
use farm::state::{new_shared_state, AppState, SharedFarmState};
use farm::db::open_merchant_db;
use farm::handlers;

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

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower == "code"
        || lower.contains("token")
        || lower.contains("authorization")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("private_key")
        || lower.contains("privatekey")
        || lower.contains("encoded")
        || lower.contains("payload")
        || lower.contains("cryptogram")
        || lower.contains("charge_bundle")
        || lower.contains("chargebundle")
        || matches!(
            lower.as_str(),
            "card_number" | "cardnumber" | "cvv" | "cvc" | "pan" | "dpan"
        )
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = Map::new();
            for (k, v) in obj {
                if is_sensitive_key(k.as_str()) {
                    out.insert(k.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(k.clone(), redact_value(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        _ => value.clone(),
    }
}

/// Health check endpoint
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "healthy"}))
}

/// List all available tools
async fn list_tools() -> Json<ToolsResponse> {
    tracing::info!("[LIST TOOLS] Received request to list available tools");

    let mut tools = vec![
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
    ];

    // Add farm tools
    for def in handlers::farm_tool_definitions() {
        tools.push(ToolDefinition {
            name: def["name"].as_str().unwrap_or("").to_string(),
            description: def["description"].as_str().unwrap_or("").to_string(),
            input_schema: def["inputSchema"].clone(),
        });
    }

    Json(ToolsResponse { tools })
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
    #[allow(dead_code)]
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
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    capabilities: serde_json::Value,
    #[serde(rename = "serverInfo")]
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

/// Dispatch farm tool calls within the MCP handler.
/// Returns Some(content) if the tool was handled, None if unknown.
async fn dispatch_farm_tool(
    tool_name: &str,
    tool_args: &serde_json::Map<String, serde_json::Value>,
    farm_state: SharedFarmState,
    merchant_db: farm::db::SharedMerchantDb,
) -> Option<Vec<serde_json::Value>> {
    let args_value = serde_json::Value::Object(tool_args.clone());

    let resp = match tool_name {
        "farm-list-products" => {
            let req: handlers::ListProductsRequest = serde_json::from_value(args_value).ok()?;
            let axum::Json(r) = handlers::handle_list_products(Json(req)).await;
            r
        }
        "farm-get-product" => {
            let req: handlers::GetProductRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_get_product(Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "farm-add-to-cart" => {
            let req: handlers::AddToCartRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_add_to_cart(State(farm_state), Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "farm-view-cart" => {
            let req: handlers::ViewCartRequest = serde_json::from_value(args_value).ok()?;
            let axum::Json(r) = handlers::handle_view_cart(State(farm_state), Json(req)).await;
            r
        }
        "farm-checkout" => {
            let req: handlers::CheckoutRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_checkout(State(farm_state), State(merchant_db), Json(req)).await {
                Ok((_, axum::Json(r))) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "pay-with-nevermined" => {
            let req: handlers::PayWithNeverminedRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_pay_with_nevermined(State(farm_state), Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "pay-with-vgs-credit-card" => {
            let req: handlers::PayWithVgsCreditCardRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_pay_with_vgs_credit_card(State(farm_state), Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "confirm-vgs-credit-card-payment" => {
            let req: handlers::ConfirmVgsCreditCardPaymentRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_confirm_vgs_credit_card_payment(State(farm_state), State(merchant_db), Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "farm-confirm-payment" => {
            let req: handlers::FarmConfirmPaymentRequest = serde_json::from_value(args_value).ok()?;
            match handlers::handle_farm_confirm_payment(State(farm_state), State(merchant_db), Json(req)).await {
                Ok(axum::Json(r)) => r,
                Err((_, axum::Json(r))) => r,
            }
        }
        "farm-clear-cart" => {
            let req: handlers::ClearCartRequest = serde_json::from_value(args_value).ok()?;
            let axum::Json(r) = handlers::handle_clear_cart(State(farm_state), Json(req)).await;
            r
        }
        _ => return None,
    };

    let text = serde_json::to_string(&resp).unwrap_or_default();
    Some(vec![json!({ "type": "text", "text": text })])
}

/// Handle MCP protocol requests
async fn handle_mcp(
    State(farm_state): State<SharedFarmState>,
    State(merchant_db): State<farm::db::SharedMerchantDb>,
    Json(req): Json<McpRequest>,
) -> Result<(StatusCode, Json<McpResponse>), (StatusCode, Json<McpResponse>)> {
    if req.method == "tools/call" {
        let tool_name = req
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("<missing>");
        let redacted_args = req
            .params
            .as_ref()
            .and_then(|p| p.get("arguments"))
            .map(redact_value)
            .unwrap_or(Value::Null);

        tracing::info!(
            "[MCP] Received request: method={}, id={:?}, tool={}, arguments={}",
            req.method,
            req.id,
            tool_name,
            redacted_args
        );
    } else {
        tracing::info!("[MCP] Received request: method={}, id={:?}", req.method, req.id);
    }

    // Per MCP spec, notifications (no id field) should return HTTP 202 Accepted with no body
    let is_notification = req.id.is_none();

    match req.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                capabilities: json!({
                    "tools": {}
                }),
                server_info: json!({
                    "name": "agent-b-mcp-server",
                    "version": "1.1.0",
                    "instructions": format!(
                        "Agent B is a travel + farm merchant MCP server.\n\
                         FLIGHT TOOLS: get-ticket-price, book-flight\n\
                         {}",
                        handlers::FARM_INSTRUCTIONS
                    ),
                }),
            };

            Ok((StatusCode::OK, Json(McpResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            })))
        }

        "tools/list" => {
            let mut tools = vec![
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

            // Add farm tool definitions
            for def in handlers::farm_tool_definitions() {
                tools.push(McpTool {
                    name: def["name"].as_str().unwrap_or("").to_string(),
                    description: def["description"].as_str().unwrap_or("").to_string(),
                    input_schema: def["inputSchema"].clone(),
                });
            }

            let result = ToolsListResult { tools };

            Ok((StatusCode::OK, Json(McpResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            })))
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

                    Ok((StatusCode::OK, Json(McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                    })))
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

                    Ok((StatusCode::OK, Json(McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(serde_json::to_value(result).unwrap()),
                        error: None,
                    })))
                }

                _ => {
                    // Try farm tools
                    let farm_result = dispatch_farm_tool(tool_name, tool_args, farm_state.clone(), merchant_db.clone()).await;
                    match farm_result {
                        Some(content) => {
                            let result = ToolCallResult {
                                content,
                                is_error: false,
                            };
                            Ok((StatusCode::OK, Json(McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: req.id,
                                result: Some(serde_json::to_value(result).unwrap()),
                                error: None,
                            })))
                        }
                        None => {
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
            }
        }

        _ => {
            // Per MCP spec, notifications (no id) should return HTTP 202 Accepted with empty body
            if is_notification {
                tracing::debug!("[MCP] Notification '{}' received and accepted", req.method);
                return Ok((StatusCode::ACCEPTED, Json(McpResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: None,
                })));
            }
            
            // Regular requests with unknown methods return 405 error
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
    println!("║      Agent B - MCP Server (Travel + Farm Merchant)         ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    // Open merchant database (SQLite)
    let merchant_db = open_merchant_db()?;

    // Check if merchant wallet is already enrolled
    if let Some(wallet) = merchant_db.get_wallet_address() {
        println!("✅ Merchant wallet: {}", wallet);
        // Set env so X402Config picks it up
        std::env::set_var("MERCHANT_WALLET_ADDRESS", &wallet);
    } else {
        println!("⚠️  Merchant wallet not enrolled — visit http://localhost:{{PORT}}/ to set up");
    }

    // Shared farm state
    let farm_state = new_shared_state();

    let app_state = AppState {
        farm: farm_state,
        merchant_db,
    };

    // Resolve static file directory (next to the binary or from STATIC_DIR env)
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| {
        // Look for static/ relative to the manifest dir (development), or next to binary
        let dev_path = concat!(env!("CARGO_MANIFEST_DIR"), "/static");
        if std::path::Path::new(dev_path).exists() {
            dev_path.to_string()
        } else {
            "./static".to_string()
        }
    });

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/tools", get(list_tools))
        .route("/tools/get-ticket-price", post(get_ticket_price))
        .route("/tools/book-flight", post(book_flight))
        .route("/tools/farm-list-products", post(handlers::handle_list_products))
        .route("/tools/farm-get-product", post(handlers::handle_get_product))
        .route("/tools/farm-add-to-cart", post(handlers::handle_add_to_cart))
        .route("/tools/farm-view-cart", post(handlers::handle_view_cart))
        .route("/tools/farm-checkout", post(handlers::handle_checkout))
        .route("/tools/pay-with-nevermined", post(handlers::handle_pay_with_nevermined))
        .route("/internal/intent-verified", get(handlers::handle_intent_verified))
        .route("/tools/pay-with-vgs-credit-card", post(handlers::handle_pay_with_vgs_credit_card))
        .route("/tools/confirm-vgs-credit-card-payment", post(handlers::handle_confirm_vgs_credit_card_payment))
        .route("/tools/farm-confirm-payment", post(handlers::handle_farm_confirm_payment))
        .route("/tools/farm-clear-cart", post(handlers::handle_clear_cart))
        .route("/farm/checkout/:order_id", get(handlers::handle_checkout_verify))
        .route("/farm/checkout-nevermined/:order_id", get(handlers::handle_checkout_nevermined))
        .route("/mcp", post(handle_mcp))
        // Merchant API
        .route("/api/merchant/status", get(handlers::handle_merchant_status))
        .route("/api/merchant/balance", get(handlers::handle_merchant_balance))
        .route("/api/merchant/send-otp", post(handlers::handle_send_otp))
        .route("/api/merchant/verify-otp", post(handlers::handle_verify_otp))
        .route("/api/products", get(handlers::handle_api_products))
        .route("/api/products/:id/chains", get(handlers::handle_get_product_chains))
        .route("/api/products/:id/chains", put(handlers::handle_set_product_chains))
        // Orders API
        .route("/api/orders", get(handlers::handle_list_orders))
        .route("/api/orders/:id/status", put(handlers::handle_update_order_status))
        // Stripe Checkout
        .route("/api/stripe/create-checkout-session", post(farm::stripe::handle_create_checkout_session))
        .route("/api/stripe/webhook", post(farm::stripe::handle_stripe_webhook))
        .route("/stripe/success", get(farm::stripe::handle_stripe_success))
        .route("/stripe/cancel", get(farm::stripe::handle_stripe_cancel))
        // Admin / Test API
        .route("/api/admin/tamper-mode", post(handlers::handle_tamper_mode))
        .route("/api/admin/tamper-mode", get(handlers::handle_tamper_status))
        // Static files (farm UI)
        .fallback_service(ServeDir::new(&static_dir))
        .with_state(app_state)
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
    println!("  GET  /                          — Farm product page");
    println!("  GET  /api/products              — Product catalog (JSON)");
    println!("  GET  /api/merchant/status        — Merchant wallet status");
    println!("  POST /api/merchant/send-otp      — Send enrollment OTP");
    println!("  POST /api/merchant/verify-otp    — Verify OTP & create wallet");
    println!("  GET  /tools                     — List all tools");
    println!("  POST /tools/get-ticket-price    — Get flight pricing");
    println!("  POST /tools/book-flight         — Book a flight");
    println!("  POST /tools/farm-list-products  — List farm products");
    println!("  POST /tools/farm-get-product    — Get product details");
    println!("  POST /tools/farm-add-to-cart    — Add to cart");
    println!("  POST /tools/farm-view-cart      — View cart");
    println!("  POST /tools/farm-checkout       — Checkout (x402/nevermined/vgs)");
    println!("  POST /tools/pay-with-nevermined — Nevermined card pay (verify → mint → settle)");
    println!("  POST /tools/pay-with-vgs-credit-card — VGS card pay via zpi-zkpay");
    println!("  GET  /farm/checkout/:order_id   — x402 payment verify");
    println!("  GET  /farm/checkout-nevermined/:order_id — Nevermined payment verify");
    println!("  POST /mcp                       — MCP protocol endpoint\n");

    // ── Nevermined config summary ─────────────────────────────────────
    let nvm_env = std::env::var("NVM_ENVIRONMENT").unwrap_or_else(|_| "sandbox (default)".to_string());
    let nvm_verify_url = match std::env::var("NEVERMINED_VERIFY_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => match nvm_env.to_lowercase().as_str() {
            "live" => "https://facilitator.nevermined.app/api/v1/x402/verify".to_string(),
            _ => "https://facilitator.sandbox.nevermined.app/api/v1/x402/verify".to_string(),
        },
    };
    let nvm_token_url = std::env::var("NEVERMINED_TOKEN_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://pay.nevermined.app/api/access-token/from-nvm-key".to_string());
    let nvm_scheme = std::env::var("NEVERMINED_SCHEME").unwrap_or_else(|_| "nvm:erc4337 (default)".to_string());
    let nvm_network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| "auto".to_string());
    let stop_flag = std::env::var("NEVERMINED_STOP_AFTER_VERIFY")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    let nvm_key_hint = match std::env::var("NVM_MERCHANT_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("NVM_API_KEY").ok().filter(|v| !v.trim().is_empty()))
    {
        Some(k) => {
            let prefix = k.split(':').next().unwrap_or("??");
            format!("set (prefix={}, legacy/local mint only)", prefix)
        }
        None => "unset (preferred — ZPI-ZKPay mints; bearer = x402 token)".to_string(),
    };
    println!("  Nevermined config:");
    println!("    NVM_ENVIRONMENT            = {}", nvm_env);
    println!("    Token mint URL             = {}", nvm_token_url);
    println!("    Facilitator verify URL     = {}", nvm_verify_url);
    println!("    NEVERMINED_SCHEME          = {}", nvm_scheme);
    println!("    NEVERMINED_NETWORK         = {}", nvm_network);
    println!("    NEVERMINED_STOP_AFTER_VERIFY = {}", stop_flag);
    println!("    Merchant NVM key           = {}", nvm_key_hint);
    println!();

    

    axum::serve(listener, app).await?;

    Ok(())
}
