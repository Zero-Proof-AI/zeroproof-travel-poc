/// HTTP Server wrapper for Agent A MCP Client
/// Exposes the orchestration logic as REST API endpoints
/// Allows web interfaces to interact with the agent

use axum::{
    extract::{Json, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use mcp_client::{AgentConfig, BookingState, ClaudeMessage, process_user_query};

/// Session data storing conversation history and booking state
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionData {
    messages: Vec<ClaudeMessage>,
    state: BookingState,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            state: BookingState::default(),
        }
    }
}

/// Session manager for storing conversation state across requests
type SessionManager = Arc<Mutex<HashMap<String, SessionData>>>;

/// Health check response
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Chat request from frontend
#[derive(Debug, Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default)]
    session_id: Option<String>,
}

/// Chat response to frontend
#[derive(Debug, Serialize)]
struct ChatResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

/// Proof submission request - sent to attestation service
#[derive(Debug, Serialize, Deserialize)]
struct ProofSubmissionRequest {
    session_id: String,
    tool_name: String,
    timestamp: u64,
    request: serde_json::Value,
    response: serde_json::Value,
    proof: serde_json::Value,
    proof_id: Option<String>,
    verified: bool,
    onchain_compatible: bool,
    #[serde(default)]
    submitted_by: Option<String>,
    #[serde(default)]
    sequence: Option<u32>,
    #[serde(default)]
    related_proof_id: Option<String>,
    #[serde(default)]
    workflow_stage: Option<String>,
}

/// Proof submission response - received from attestation service
#[derive(Debug, Serialize, Deserialize)]
struct ProofSubmissionResponse {
    success: bool,
    proof_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Proofs retrieval response - passed through from attester service
#[derive(Debug, Serialize, Deserialize)]
struct ProofsResponse {
    success: bool,
    session_id: String,
    count: usize,
    proofs: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_metadata: Option<VerificationMetadata>,
}

/// Metadata to help agents verify proofs on-chain
#[derive(Debug, Serialize, Deserialize)]
struct VerificationMetadata {
    proof_protocol: Option<String>,
    verification_method: Option<String>,
    contract_chain: Option<String>,
    contract_address: Option<String>,
    documentation_url: Option<String>,
}

/// Single proof response - optimized for on-chain verification
#[derive(Debug, Serialize, Deserialize)]
struct SingleProofResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: "0.1.0".to_string(),
    })
}

/// Helper to get attestation service URL from environment or default
fn get_attestation_service_url() -> String {
    std::env::var("ATTESTATION_SERVICE_URL")
        .unwrap_or_else(|_| "http://localhost:8000".to_string())
}

/// Helper to get attestation service URL and HTTP client together
fn get_attestation_client() -> (String, reqwest::Client) {
    (get_attestation_service_url(), reqwest::Client::new())
}

/// Main chat endpoint with session-based conversation management
async fn chat(
    axum::extract::Extension(sessions): axum::extract::Extension<SessionManager>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let session_id = payload.session_id.clone().unwrap_or_else(|| {
        format!("sess_{}", uuid::Uuid::new_v4())
    });
    
    println!("[CHAT] Incoming request - SessionID: {}, Message length: {}", 
             session_id, payload.message.len());
    
    // Initialize config
    let config = match AgentConfig::from_env() {
        Ok(cfg) => {
            println!("[CONFIG] Configuration loaded successfully");
            cfg
        }
        Err(e) => {
            println!("[ERROR] Configuration failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ChatResponse {
                    success: false,
                    response: None,
                    error: Some(format!("Configuration error: {}", e)),
                    session_id: Some(session_id),
                }),
            );
        }
    };

    // Lock session manager and get/create session data
    let mut sessions_lock = sessions.lock().await;
    let mut session = sessions_lock
        .get(&session_id)
        .cloned()
        .unwrap_or_default();
    
    println!("[SESSION] Retrieved session - State: {}, Messages: {}", 
             session.state.step, session.messages.len());

    // Process the user query with full conversation history and state
    println!("[PROCESSING] Starting orchestration - User message: '{}'", &payload.message[..payload.message.len().min(100)]);
    
    match process_user_query(&config, &payload.message, &session.messages, &mut session.state, &session_id).await {
        Ok((response, updated_messages, updated_state)) => {
            // Update session with new messages and state
            let message_count = updated_messages.len();
            session.messages = updated_messages;
            
            session.state = updated_state.clone();
            sessions_lock.insert(session_id.clone(), session);
            
            println!("[SUCCESS] Request processed - SessionID: {}, New state: {}, Messages: {}", 
                     session_id, updated_state.step, message_count);
            
            (
                StatusCode::OK,
                Json(ChatResponse {
                    success: true,
                    response: Some(response),
                    error: None,
                    session_id: Some(session_id),
                }),
            )
        }
        Err(e) => {
            println!("[ERROR] Processing failed - SessionID: {}, Error: {}", session_id, e);
            (
                StatusCode::BAD_REQUEST,
                Json(ChatResponse {
                    success: false,
                    response: None,
                    error: Some(format!("Error processing request: {}", e)),
                    session_id: Some(session_id),
                }),
            )
        }
    }
}

/// Submit a cryptographic proof - proxy to zk-attestation-service
async fn submit_proof(
    Json(payload): Json<ProofSubmissionRequest>,
) -> impl IntoResponse {
    println!("[AGENT-A PROOF SUBMIT] 📤 Received proof submission");
    println!("  tool_name: {}", payload.tool_name);
    println!("  session_id: {}", payload.session_id);
    println!("  verified: {}", payload.verified);
    println!("  onchain_compatible: {}", payload.onchain_compatible);
    println!("  workflow_stage: {:?}", payload.workflow_stage);
    
    let (attestation_url, client) = get_attestation_client();
    
    println!("[AGENT-A PROOF SUBMIT] Calling attestation service at: {}/proofs/submit", attestation_url);
    
    let submit_url = format!("{}/proofs/submit", attestation_url);
    
    match client.post(&submit_url).json(&payload).send().await {
        Ok(response) => {
            println!("[AGENT-A PROOF SUBMIT] ✓ Got response from attestation service (status: {})", response.status());
            
            match response.json::<ProofSubmissionResponse>().await {
                Ok(result) => {
                    println!("[AGENT-A PROOF SUBMIT] ✅ Proof submitted successfully: proof_id={:?}", result.proof_id);
                    (StatusCode::OK, Json(result))
                }
                Err(e) => {
                    println!("[AGENT-A PROOF SUBMIT] ❌ Failed to parse attestation response: {}", e);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(ProofSubmissionResponse {
                            success: false,
                            proof_id: String::new(),
                            error: Some(format!("Failed to parse response: {}", e)),
                        }),
                    )
                }
            }
        }
        Err(e) => {
            println!("[AGENT-A PROOF SUBMIT] ❌ Failed to call attestation service at {}: {}", submit_url, e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ProofSubmissionResponse {
                    success: false,
                    proof_id: String::new(),
                    error: Some(format!("Attestation service error: {}", e)),
                }),
            )
        }
    }
}

/// Retrieve proofs for a session - proxy to zk-attestation-service
async fn get_proofs(
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    println!("[AGENT-A PROOFS LIST] 📋 Retrieving proofs for session: {}", session_id);
    
    let (attestation_url, client) = get_attestation_client();
    let fetch_url = format!("{}/proofs/session/{}", attestation_url, session_id);
    
    println!("[AGENT-A PROOFS LIST] Calling attestation service: {}", fetch_url);
    
    match client.get(&fetch_url).send().await {
        Ok(response) => {
            println!("[AGENT-A PROOFS LIST] ✓ Got response from attestation service (status: {})", response.status());
            
            match response.json::<serde_json::Value>().await {
                Ok(result) => {
                    let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("[AGENT-A PROOFS LIST] ✅ Retrieved {} proofs from attestation service", count);
                    (StatusCode::OK, Json(result))
                }
                Err(e) => {
                    println!("[AGENT-A PROOFS LIST] ❌ Failed to parse attestation response: {}", e);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "success": false,
                            "error": format!("Failed to parse response: {}", e)
                        })),
                    )
                }
            }
        }
        Err(e) => {
            println!("[AGENT-A PROOFS LIST] ❌ Failed to call attestation service: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Attestation service error: {}", e)
                })),
            )
        }
    }
}

/// Get proof count for a session - proxy to zk-attestation-service
async fn get_proof_count(
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    println!("[AGENT-A PROOF COUNT] 📊 Getting proof count for session: {}", session_id);
    
    let (attestation_url, client) = get_attestation_client();
    let fetch_url = format!("{}/proofs/count/{}", attestation_url, session_id);
    
    println!("[AGENT-A PROOF COUNT] Calling attestation service: {}", fetch_url);
    
    match client.get(&fetch_url).send().await {
        Ok(response) => {
            println!("[AGENT-A PROOF COUNT] ✓ Got response from attestation service (status: {})", response.status());
            
            match response.json::<serde_json::Value>().await {
                Ok(result) => {
                    let count = result.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("[AGENT-A PROOF COUNT] ✅ Session has {} proofs", count);
                    (StatusCode::OK, Json(result))
                }
                Err(e) => {
                    println!("[AGENT-A PROOF COUNT] ❌ Failed to parse attestation response: {}", e);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "success": false,
                            "error": format!("Failed to parse response: {}", e)
                        })),
                    )
                }
            }
        }
        Err(e) => {
            println!("[AGENT-A PROOF COUNT] ❌ Failed to call attestation service: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Attestation service error: {}", e)
                })),
            )
        }
    }
}

/// Retrieve a single proof by ID - proxy to zk-attestation-service
async fn get_proof_by_id(
    Path(proof_id): Path<String>,
) -> impl IntoResponse {
    println!("[AGENT-A PROOF GET] 🔍 Retrieving proof for verification: {}", proof_id);
    
    let (attestation_url, client) = get_attestation_client();
    let fetch_url = format!("{}/proofs/{}", attestation_url, proof_id);
    
    println!("[AGENT-A PROOF GET] Calling attestation service: {}", fetch_url);
    
    match client.get(&fetch_url).send().await {
        Ok(response) => {
            println!("[AGENT-A PROOF GET] ✓ Got response from attestation service (status: {})", response.status());
            
            match response.json::<serde_json::Value>().await {
                Ok(result) => {
                    let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                    if success {
                        println!("[AGENT-A PROOF GET] ✅ Retrieved proof successfully");
                        // Flatten response: extract 'data' field and make it top-level 'proof'
                        let proof = result.get("data").and_then(|d| d.get("proof")).cloned().unwrap_or_default();
                        let verification_info = result.get("data").and_then(|d| d.get("verification_info")).cloned();
                        
                        let flattened_response = serde_json::json!({
                            "success": true,
                            "proof": proof,
                            "verification_info": verification_info
                        });
                        (StatusCode::OK, Json(flattened_response))
                    } else {
                        println!("[AGENT-A PROOF GET] ⚠️ Proof not found or error in response");
                        (StatusCode::OK, Json(result))
                    }
                }
                Err(e) => {
                    println!("[AGENT-A PROOF GET] ❌ Failed to parse attestation response: {}", e);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "success": false,
                            "error": format!("Failed to parse response: {}", e)
                        })),
                    )
                }
            }
        }
        Err(e) => {
            println!("[AGENT-A PROOF GET] ❌ Failed to call attestation service: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Attestation service error: {}", e)
                })),
            )
        }
    }
}

#[tokio::main]
async fn main() {
    // Load .env file
    let _ = dotenv::dotenv();

    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║     Agent A - HTTP API Server (Claude-powered Agent)       ║");
    println!("║    With Session-based Conversation Management              ║");
    println!("║   And Distributed Proof Collection Infrastructure          ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");

    // Get port from environment or use default
    let port = std::env::var("AGENT_A_HTTP_PORT")
        .unwrap_or_else(|_| "3001".to_string())
        .parse::<u16>()
        .unwrap_or(3001);
    
    println!("[INIT] Server configuration:");
    println!("  Port: {}", port);
    
    if let Ok(agent_b_url) = std::env::var("AGENT_B_MCP_URL") {
        println!("  Agent B URL: {}", agent_b_url);
    }
    
    if let Ok(payment_url) = std::env::var("PAYMENT_AGENT_URL") {
        println!("  Payment Agent URL: {}", payment_url);
    }
    
    if let Ok(zkfetch_url) = std::env::var("ZKFETCH_WRAPPER_URL") {
        println!("  zkfetch-wrapper URL: {}", zkfetch_url);
    } else {
        println!("  zkfetch-wrapper URL: NOT CONFIGURED - proofs will NOT be collected");
    }
    
    if let Ok(attestation_url) = std::env::var("ATTESTATION_SERVICE_URL") {
        println!("  Attestation Service URL: {}", attestation_url);
    } else {
        println!("  Attestation Service URL: http://localhost:8000 (default)");
    }

    // Create session manager
    let sessions: SessionManager = Arc::new(Mutex::new(HashMap::new()));
    println!("[SESSION] Session manager initialized");
    println!("[PROOF] All proofs proxied to zk-attestation-service");

    // Build router with session manager extension
    let app = Router::new()
        .route("/health", get(health))
        .route("/chat", post(chat))
        .route("/proofs", post(submit_proof))
        .route("/proofs/verify/:proof_id", get(get_proof_by_id))  // Most specific first
        .route("/proofs/:session_id/count", get(get_proof_count)) // Second most specific
        .route("/proofs/:session_id", get(get_proofs))             // Least specific (catch-all)
        .layer(CorsLayer::permissive())
        .layer(axum::extract::Extension(sessions));

    // Create listener
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind listener");

    println!("[STARTUP] ✓ Agent A HTTP Server running on http://0.0.0.0:{}", port);
    println!("  POST /chat                      — Send a message (session-based conversation)");
    println!("  POST /proofs                    — Submit a cryptographic proof");
    println!("  GET  /proofs/:session_id        — Get all proofs for session (with full ZK-TLS data)");
    println!("  GET  /proofs/:session_id/count  — Get proof count for session");
    println!("  GET  /proofs/verify/:proof_id   — Get proof by ID (optimized for on-chain verification)");
    println!("  GET  /health                    — Check server health");
    println!("\n✅ Proof Verification - Any agent can independently verify proofs against Reclaim smart contract");
    println!("✅ Full proof data includes: ZK-TLS proof, request/response, workflow metadata\n");

    // Run server
    if let Err(e) = axum::serve(listener, app).await {
        println!("[FATAL] Server failed: {}", e);
    }
}
