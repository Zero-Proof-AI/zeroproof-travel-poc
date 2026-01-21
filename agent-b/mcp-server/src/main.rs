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
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use pricing_core::pricing;
use pricing_core::booking;

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
    inputSchema: serde_json::Value,
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
                inputSchema: json!({
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
                inputSchema: json!({
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
    
    // Validate input
    if req.from.is_empty() || req.to.is_empty() {
        tracing::warn!("[GET-TICKET-PRICE] Validation failed: missing required fields");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(
                "from and to fields are required".to_string(),
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
    
    // Validate input
    if req.from.is_empty() || req.to.is_empty() || req.passenger_name.is_empty() {
        tracing::warn!("[BOOK-FLIGHT] Validation failed: missing required fields");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(tool_error(
                "from, to, and passenger_name are required".to_string(),
            )),
        ));
    }

    // Use pricing-core to generate booking (async version that calls httpbin.org)
    let core_req = booking::Request {
        from: req.from.clone(),
        to: req.to.clone(),
        passenger_name: req.passenger_name.clone(),
        passenger_email: req.passenger_email.clone(),
    };

    let core_resp = booking::handle_async(core_req).await;
    
    tracing::info!("[BOOK-FLIGHT] result: booking_id={}, confirmation_code={}, status={}", core_resp.booking_id, core_resp.confirmation_code, core_resp.status);

    Ok(Json(ToolResponse::ok(BookResponse {
        booking_id: core_resp.booking_id,
        status: core_resp.status,
        confirmation_code: core_resp.confirmation_code,
        from: req.from,
        to: req.to,
        passenger_name: req.passenger_name,
    })))
}

#[tokio::main]
async fn main() -> Result<()> {
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

    axum::serve(listener, app).await?;

    Ok(())
}
