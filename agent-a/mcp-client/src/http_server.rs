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

use mcp_client::{AgentConfig, BookingState, ClaudeMessage, process_user_query, ProofDatabase, StoredProof};

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

/// Proof submission request
#[derive(Debug, Deserialize)]
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

/// Proof submission response
#[derive(Debug, Serialize)]
struct ProofSubmissionResponse {
    success: bool,
    proof_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Proofs retrieval response - with full on-chain verification data
#[derive(Debug, Serialize)]
struct ProofsResponse {
    success: bool,
    session_id: String,
    count: usize,
    proofs: Vec<StoredProof>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_metadata: Option<VerificationMetadata>,
}

/// Metadata to help agents verify proofs on-chain
#[derive(Debug, Serialize)]
struct VerificationMetadata {
    proof_protocol: String,                    // "ZK-TLS" or "Reclaim Protocol"
    verification_method: String,               // How to verify (e.g., "Reclaim Smart Contract")
    contract_chain: Option<String>,            // Network where contract is deployed
    contract_address: Option<String>,          // Smart contract address for verification
    documentation_url: Option<String>,         // Link to verification documentation
}

/// Single proof response - optimized for on-chain verification
#[derive(Debug, Serialize)]
struct SingleProofResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<SingleProofData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Complete proof data for on-chain submission
#[derive(Debug, Serialize, Clone)]
struct SingleProofData {
    // Core proof data
    proof_id: String,
    session_id: String,
    tool_name: String,
    timestamp: u64,
    
    // Original request/response
    request: serde_json::Value,
    response: serde_json::Value,
    
    // ZK-TLS proof (most important for on-chain verification)
    proof: serde_json::Value,
    
    // Verification status
    verified: bool,
    onchain_compatible: bool,
    
    // Workflow context
    submitted_by: Option<String>,
    sequence: Option<u32>,
    related_proof_id: Option<String>,
    workflow_stage: Option<String>,
    
    // Metadata for verification
    verification_info: VerificationInfo,
}

/// Verification information for any agent to verify proof on-chain
#[derive(Debug, Serialize, Clone)]
struct VerificationInfo {
    protocol: String,                    // "ZK-TLS"
    issuer: String,                      // "Reclaim Protocol"
    timestamp_verified: bool,            // Was timestamp verified
    signature_algorithm: String,         // Algorithm used for signing
    can_verify_onchain: bool,           // Is this proof ready for blockchain
    reclaim_documentation: String,       // Link to how to verify with Reclaim
}
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: "0.1.0".to_string(),
    })
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

/// Submit a cryptographic proof
async fn submit_proof(
    axum::extract::Extension(proof_db): axum::extract::Extension<ProofDatabase>,
    Json(payload): Json<ProofSubmissionRequest>,
) -> impl IntoResponse {
    println!("[PROOF] Received proof submission: tool={}, session={}, stage={:?}", 
             payload.tool_name, payload.session_id, payload.workflow_stage);
    
    let proof_id = payload.proof_id.clone().unwrap_or_else(|| {
        format!("proof_{}", uuid::Uuid::new_v4())
    });
    
    let stored_proof = StoredProof {
        proof_id: proof_id.clone(),
        session_id: payload.session_id,
        tool_name: payload.tool_name,
        timestamp: payload.timestamp,
        request: payload.request,
        response: payload.response,
        proof: payload.proof,
        verified: payload.verified,
        onchain_compatible: payload.onchain_compatible,
        submitted_by: payload.submitted_by,
        sequence: payload.sequence,
        related_proof_id: payload.related_proof_id,
        workflow_stage: payload.workflow_stage,
        display_response: None,  // Can be populated separately if needed
        redaction_metadata: None,  // Can be populated separately if needed
    };
    
    match proof_db.store_proof(stored_proof).await {
        Ok(stored_id) => {
            println!("[PROOF] Proof stored: {}", stored_id);
            (
                StatusCode::OK,
                Json(ProofSubmissionResponse {
                    success: true,
                    proof_id: stored_id,
                    error: None,
                }),
            )
        }
        Err(e) => {
            println!("[ERROR] Failed to store proof: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ProofSubmissionResponse {
                    success: false,
                    proof_id: String::new(),
                    error: Some(e),
                }),
            )
        }
    }
}

/// Retrieve proofs for a session
async fn get_proofs(
    axum::extract::Extension(proof_db): axum::extract::Extension<ProofDatabase>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    println!("[PROOF] Retrieving full proofs for session: {}", session_id);
    
    match proof_db.get_proofs(&session_id).await {
        Ok(proofs) => {
            let count = proofs.len();
            
            // Log detailed proof data for debugging
            for proof in &proofs {
                println!("[PROOF] Proof details - ID: {}, Tool: {}, Verified: {}, On-chain: {}, Stage: {:?}", 
                         proof.proof_id, 
                         proof.tool_name, 
                         proof.verified,
                         proof.onchain_compatible,
                         proof.workflow_stage);
                
                // Log raw proof structure for verification
                // if let Ok(proof_str) = serde_json::to_string_pretty(&proof.proof) {
                //     println!("[PROOF] Raw ZK-TLS proof structure:\n{}", proof_str);
                // }
            }
            
            // Create verification metadata for cross-agent verification
            let verification_metadata = VerificationMetadata {
                proof_protocol: "ZK-TLS (Reclaim Protocol)".to_string(),
                verification_method: "Reclaim Smart Contract Verification".to_string(),
                contract_chain: Some("Ethereum Sepolia".to_string()),
                contract_address: Some("0x0000000000000000000000000000000000000000".to_string()), // Placeholder
                documentation_url: Some("https://docs.reclaim.ai/verify".to_string()),
            };
            
            (
                StatusCode::OK,
                Json(ProofsResponse {
                    success: true,
                    session_id,
                    count,
                    proofs,
                    verification_metadata: Some(verification_metadata),
                }),
            )
        }
        Err(e) => {
            println!("[ERROR] Failed to retrieve proofs: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ProofsResponse {
                    success: false,
                    session_id,
                    count: 0,
                    proofs: Vec::new(),
                    verification_metadata: None,
                }),
            )
        }
    }
}

/// Get proof count for a session
async fn get_proof_count(
    axum::extract::Extension(proof_db): axum::extract::Extension<ProofDatabase>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match proof_db.get_proof_count(&session_id).await {
        Ok(count) => {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "session_id": session_id,
                    "count": count
                })),
            )
        }
        Err(e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e
                })),
            )
        }
    }
}

/// Retrieve a single proof by ID with full on-chain verification metadata
async fn get_proof_by_id(
    axum::extract::Extension(proof_db): axum::extract::Extension<ProofDatabase>,
    Path(proof_id): Path<String>,
) -> impl IntoResponse {
    println!("[PROOF] Retrieving proof for on-chain verification: {}", proof_id);
    
    match proof_db.get_proof(&proof_id).await {
        Ok(Some(stored_proof)) => {
            println!("[PROOF] Found proof - Tool: {}, Verified: {}, On-chain: {}", 
                     stored_proof.tool_name, 
                     stored_proof.verified,
                     stored_proof.onchain_compatible);
            
            let single_proof = SingleProofData {
                proof_id: stored_proof.proof_id.clone(),
                session_id: stored_proof.session_id.clone(),
                tool_name: stored_proof.tool_name.clone(),
                timestamp: stored_proof.timestamp,
                request: stored_proof.request.clone(),
                response: stored_proof.response.clone(),
                proof: stored_proof.proof.clone(),
                verified: stored_proof.verified,
                onchain_compatible: stored_proof.onchain_compatible,
                submitted_by: stored_proof.submitted_by.clone(),
                sequence: stored_proof.sequence,
                related_proof_id: stored_proof.related_proof_id.clone(),
                workflow_stage: stored_proof.workflow_stage.clone(),
                verification_info: VerificationInfo {
                    protocol: "ZK-TLS".to_string(),
                    issuer: "Reclaim Protocol".to_string(),
                    timestamp_verified: stored_proof.verified,
                    signature_algorithm: "SHA256withECDSA".to_string(),
                    can_verify_onchain: stored_proof.onchain_compatible,
                    reclaim_documentation: "https://docs.reclaim.ai/verify".to_string(),
                },
            };
            
            (
                StatusCode::OK,
                Json(SingleProofResponse {
                    success: true,
                    proof: Some(single_proof),
                    error: None,
                }),
            )
        }
        Ok(None) => {
            println!("[WARNING] Proof not found: {}", proof_id);
            (
                StatusCode::NOT_FOUND,
                Json(SingleProofResponse {
                    success: false,
                    proof: None,
                    error: Some(format!("Proof not found: {}", proof_id)),
                }),
            )
        }
        Err(e) => {
            println!("[ERROR] Failed to retrieve proof: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SingleProofResponse {
                    success: false,
                    proof: None,
                    error: Some(e),
                }),
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

    // Create session manager
    let sessions: SessionManager = Arc::new(Mutex::new(HashMap::new()));
    println!("[SESSION] Session manager initialized");

    // Create proof database
    let proof_db = ProofDatabase::new();
    println!("[PROOF] Proof database initialized");

    // Build router with session manager and proof database extensions
    let app = Router::new()
        .route("/health", get(health))
        .route("/chat", post(chat))
        .route("/proofs", post(submit_proof))
        .route("/proofs/:session_id", get(get_proofs))
        .route("/proofs/:session_id/count", get(get_proof_count))
        .route("/proofs/verify/:proof_id", get(get_proof_by_id))
        .layer(CorsLayer::permissive())
        .layer(axum::extract::Extension(sessions))
        .layer(axum::extract::Extension(proof_db));

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
