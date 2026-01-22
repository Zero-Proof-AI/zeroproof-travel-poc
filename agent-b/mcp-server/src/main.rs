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

use pricing_core::pricing;
use pricing_core::booking;
use shared::proof::CryptographicProof;

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
    })))
}

/// Book a flight
async fn book_flight(
    Json(req): Json<BookRequest>,
) -> Result<Json<ToolResponse<BookResponse>>, (StatusCode, Json<ToolResponse<()>>)> {
    tracing::info!("[BOOK-FLIGHT] Tool call received: from={}, to={}, passenger={}, email={}", req.from, req.to, req.passenger_name, req.passenger_email);
    
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

    // Get zkfetch URL from environment
    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
        .unwrap_or_else(|_| "http://localhost:8003".to_string());

    // Session_id must be provided by agent-a for proof tracking across the workflow
    let session_id = req.session_id.clone()
        .ok_or_else(|| {
            tracing::warn!("[BOOK-FLIGHT] Missing required field: session_id");
            (
                StatusCode::BAD_REQUEST,
                Json(tool_error("Missing required field: 'session_id' (must be provided by orchestrator)".to_string())),
            )
        })?;

    // Delegate to pricing-core library handle_async for business logic
    let core_req = booking::Request {
        from: req.from.clone(),
        to: req.to.clone(),
        passenger_name: req.passenger_name.clone(),
        passenger_email: req.passenger_email.clone(),
    };

    let (response, proof_value) = booking::handle_async(core_req, zkfetch_url, &session_id).await;

    tracing::info!("[BOOK-FLIGHT] result: booking_id={}, confirmation_code={}, status={}", response.booking_id, response.confirmation_code, response.status);

    // If we received a proof, submit it to the attestation service asynchronously
    if let Some(proof_json) = proof_value {
        
        // Clone values needed for async task
        let response_clone = BookResponse {
            booking_id: response.booking_id.clone(),
            status: response.status.clone(),
            confirmation_code: response.confirmation_code.clone(),
            from: req.from.clone(),
            to: req.to.clone(),
            passenger_name: req.passenger_name.clone(),
        };
        
        let req_clone = BookRequest {
            from: req.from.clone(),
            to: req.to.clone(),
            passenger_name: req.passenger_name.clone(),
            passenger_email: req.passenger_email.clone(),
            session_id: req.session_id.clone(),
        };
        
        tokio::spawn(async move {
            // Create CryptographicProof structure from the zkfetch proof
            let crypto_proof = CryptographicProof {
                tool_name: "book-flight".to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                request: serde_json::json!({
                    "from": req_clone.from,
                    "to": req_clone.to,
                    "passenger_name": req_clone.passenger_name,
                    "passenger_email": req_clone.passenger_email,
                }),
                response: serde_json::json!({
                    "booking_id": response_clone.booking_id,
                    "status": response_clone.status,
                    "confirmation_code": response_clone.confirmation_code,
                }),
                proof: proof_json,
                proof_id: None,
                verified: false,
                onchain_compatible: false,
                display_response: None,
                redaction_metadata: None,
            };

            // Get attestation service URL from environment
            let attestation_service_url = std::env::var("ATTESTATION_SERVICE_URL")
                .unwrap_or_else(|_| "http://localhost:3001".to_string());
            
            match shared::submit_proof(
                &attestation_service_url,
                &session_id,
                &crypto_proof,
                None,  // sequence
                None,  // related_proof_id
                Some("booking".to_string()),  // workflow_stage
                None,  // progress_tx
            ).await {
                Ok(proof_id) => {
                    tracing::info!("[BOOK-FLIGHT] ✓ Proof submitted to attestation service: {}", proof_id);
                }
                Err(e) => {
                    tracing::warn!("[BOOK-FLIGHT] Failed to submit proof: {}", e);
                }
            }
        });
    } else {
        tracing::warn!("[BOOK-FLIGHT] No proof received from booking operation");
    }

    Ok(Json(ToolResponse::ok(BookResponse {
        booking_id: response.booking_id,
        status: response.status,
        confirmation_code: response.confirmation_code,
        from: req.from,
        to: req.to,
        passenger_name: req.passenger_name,
    })))
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
        .layer(CorsLayer::permissive());

    // Get port from environment variable or use default
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8001".to_string())
        .parse::<u16>()?;
    let addr = format!("0.0.0.0:{}", port);

    // Bind and serve
    let listener = tokio::net::TcpListener::bind(&addr)
        .await?;

    println!("✓ Agent B MCP Server running on http://0.0.0.0:{}", port);
    println!("  GET  /tools                     — List all tools");
    println!("  POST /tools/get-ticket-price    — Get flight pricing");
    println!("  POST /tools/book-flight         — Book a flight\n");

    // Print zkfetch endpoint configuration
    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
        .unwrap_or_else(|_| "http://localhost:8003".to_string());
    println!("🔐 zkfetch Endpoint: {}/zkfetch\n", zkfetch_url);

    axum::serve(listener, app).await?;

    Ok(())
}
