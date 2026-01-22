use axum::{
    extract::State,
    routing::post,
    Router, Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::sync::Arc;
use pricing_core::{pricing, booking};

mod zk_adapter;

#[derive(Deserialize)]
struct PriceRequest {
    from: String,
    to: String,
    vip: bool,
}

#[derive(Serialize)]
struct PriceResponse {
    // Agent-specific data
    price: f64,
    // ZK verification metadata
    program_id: String,
    elf_hash: String,
}

use serde_json::Value;

#[derive(Serialize)]
struct BookResponse {
    // Agent-specific data
    booking_id: String,
    status: String,
    confirmation_code: String,
    // ZK verification metadata
    program_id: String,
    elf_hash: String,
    // Optional ZK proof from zkfetch
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<Value>,
}

#[derive(Deserialize)]
struct BookRequest {
    from: String,
    to: String,
    passenger_name: String,
    passenger_email: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Clone)]
struct AppState {
    program_id: String,
    elf_hash: String,
    booking_api_url: Option<String>,
}

async fn price_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PriceRequest>,
) -> Json<PriceResponse> {
    // Use pricing-core logic
    let core_req = pricing::Request {
        from: req.from,
        to: req.to,
        vip: req.vip,
    };
    
    let core_resp = pricing::handle(core_req);

    Json(PriceResponse {
        price: core_resp.price,
        program_id: state.program_id.clone(),
        elf_hash: state.elf_hash.clone(),
    })
}

async fn fallback_booking(req: &BookRequest) -> (booking::Response, Option<Value>) {
    let core_req = booking::Request {
        from: req.from.clone(),
        to: req.to.clone(),
        passenger_name: req.passenger_name.clone(),
        passenger_email: req.passenger_email.clone(),
    };
    let zkfetch_url = std::env::var("ZKFETCH_WRAPPER_URL")
        .unwrap_or_else(|_| "http://localhost:8003".to_string());
    // Session_id must be provided - use what's passed or return empty response
    match &req.session_id {
        Some(session_id) => booking::handle_async(core_req, zkfetch_url, session_id).await,
        None => {
            // Return empty response if session_id not provided
            eprintln!("[FALLBACK_BOOKING] Missing session_id in request");
            (
                booking::Response {
                    booking_id: String::new(),
                    status: "failed".to_string(),
                    confirmation_code: String::new(),
                },
                None,
            )
        }
    }
}

async fn book_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BookRequest>,
) -> Json<BookResponse> {
    // If BOOKING_API_URL is set, try calling the real API with fallback
    let (core_resp, proof) = if let Some(api_url) = &state.booking_api_url {
        match call_booking_api(api_url, &req).await {
            Ok(resp) => (resp, None),
            Err(e) => {
                eprintln!("⚠ Booking API call failed: {}, using fallback", e);
                fallback_booking(&req).await
            }
        }
    } else {
        // Use fallback async booking logic directly
        fallback_booking(&req).await
    };

    Json(BookResponse {
        booking_id: core_resp.booking_id,
        status: core_resp.status,
        confirmation_code: core_resp.confirmation_code,
        program_id: state.program_id.clone(),
        elf_hash: state.elf_hash.clone(),
        proof,
    })
}

async fn call_booking_api(
    api_url: &str,
    req: &BookRequest,
) -> Result<booking::Response, String> {
    let client = reqwest::Client::new();
    
    #[derive(Serialize)]
    struct ApiRequest {
        from: String,
        to: String,
        passenger_name: String,
        passenger_email: String,
    }
    
    let api_req = ApiRequest {
        from: req.from.clone(),
        to: req.to.clone(),
        passenger_name: req.passenger_name.clone(),
        passenger_email: req.passenger_email.clone(),
    };
    
    let response = client
        .post(api_url)
        .json(&api_req)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;
    
    #[derive(Deserialize)]
    struct ApiResponse {
        booking_id: String,
        status: String,
        confirmation_code: String,
    }
    
    let api_resp: ApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse API response: {}", e))?;
    
    Ok(booking::Response {
        booking_id: api_resp.booking_id,
        status: api_resp.status,
        confirmation_code: api_resp.confirmation_code,
    })
}

async fn register_elf_with_attester(
    elf_bytes: Vec<u8>,
    attester_url: &str,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(elf_bytes)
        .file_name("agent-b-program.elf")
        .mime_str("application/octet-stream")
        .map_err(|e| format!("Failed to create multipart: {}", e))?;
    
    let form = reqwest::multipart::Form::new()
        .part("elf", part);

    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/register-elf", attester_url))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Failed to register ELF: {}", e))?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse attester response: {}", e))?;

    body["program_id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No program_id in response".to_string())
}

// POST /zk-input - Helper endpoint for external agents
// Returns properly formatted zkVM input bytes
#[derive(Deserialize)]
struct ZkInputRequest {
    endpoint: String,  // "price" or "book"
    input: serde_json::Value,
}

#[derive(Serialize)]
struct ZkInputResponse {
    input_bytes: Vec<u8>,
}

async fn zk_input_handler(
    Json(req): Json<ZkInputRequest>,
) -> Json<ZkInputResponse> {
    let rpc_call = zk_adapter::json_to_rpc_call(&req.endpoint, &req.input)
        .expect("Failed to convert to RpcCall");
    
    let input_bytes = zk_adapter::rpc_call_to_bytes(&rpc_call);
    
    Json(ZkInputResponse { input_bytes })
}

#[tokio::main]
async fn main() {
    let attester_url = std::env::var("ATTESTER_URL")
        .unwrap_or_else(|_| "http://localhost:8000".to_string());

    // Read the proper ELF binary (not .a archive)
    let elf_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/elf-compilation/riscv32im-succinct-zkvm-elf/release/agent-b-program");
    
    println!("Loading ELF from: {:?}", elf_path);
    let elf_bytes = std::fs::read(&elf_path)
        .expect(&format!("Failed to read {:?}. Run 'cd program && cargo prove build' first.", elf_path));

    // Compute ELF hash
    let mut hasher = Sha256::new();
    hasher.update(&elf_bytes);
    let elf_hash = format!("0x{}", hex::encode(hasher.finalize()));

    // Register with attester
    let program_id = register_elf_with_attester(elf_bytes, &attester_url)
        .await
        .expect("Failed to register ELF with attester");

    println!("✓ ELF registered with attester");
    println!("  program_id: {}", program_id);
    println!("  elf_hash: {}", elf_hash);
    println!("  attester_url: {}", attester_url);

    // Optional: External booking API URL
    let booking_api_url = std::env::var("BOOKING_API_URL").ok();
    if let Some(ref url) = booking_api_url {
        println!("  booking_api_url: {}", url);
    } else {
        println!("  booking_api_url: (not set, using deterministic logic)");
    }

    let state = Arc::new(AppState {
        program_id,
        elf_hash,
        booking_api_url,
    });

    let app = Router::new()
        .route("/price", post(price_handler))
        .route("/book", post(book_handler))
        .route("/zk-input", post(zk_input_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8001")
        .await
        .expect("Failed to bind to 0.0.0.0:8001");

    println!("✓ Agent B running on http://0.0.0.0:8001");
    println!("  POST /price  — Get flight pricing");
    println!("  POST /book   — Book a flight");

    axum::serve(listener, app)
        .await
        .expect("Server error");
}