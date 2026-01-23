/// HTTP Server wrapper for Agent A MCP Client
/// Exposes the orchestration logic as REST API endpoints and WebSocket connections
/// Allows web interfaces to interact with the agent in real-time

use axum::{
    extract::{ws::{WebSocketUpgrade, WebSocket, Message}, Json, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
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
    std::env::var("ATTESTER_URL")
        .unwrap_or_else(|_| "https://dev.attester.zeroproofai.com".to_string())
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
    
    println!("[CHAT] 📨 Incoming request");
    println!("[CHAT]   SessionID: {}", session_id);
    println!("[CHAT]   Message: {}", payload.message);
    println!("[CHAT]   Message length: {}", payload.message.len());
    
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
    println!("[SESSION] Acquiring lock for session: {}", session_id);
    let mut sessions_lock = sessions.lock().await;
    let mut session = sessions_lock
        .get(&session_id)
        .cloned()
        .unwrap_or_default();
    
    println!("[SESSION] ✓ Lock acquired. Retrieved session - State: {}, Messages: {}", 
             session.state.step, session.messages.len());
    
    // IMPORTANT: Release lock BEFORE processing to avoid deadlocks
    drop(sessions_lock);
    println!("[SESSION] Lock released before calling process_user_query");

    // Process the user query with full conversation history and state
    println!("[PROCESSING] Starting orchestration - User message: '{}'", &payload.message[..payload.message.len().min(100)]);
    
    match process_user_query(&config, &payload.message, &session.messages, &mut session.state, &session_id, None).await {
        Ok((response, updated_messages, updated_state)) => {
            println!("[PROCESSING] ✓ Orchestration completed successfully");
            // Update session with new messages and state
            let message_count = updated_messages.len();
            session.messages = updated_messages;
            
            session.state = updated_state.clone();
            
            // Re-acquire lock to update session
            println!("[SESSION] Re-acquiring lock to save updated session");
            let mut sessions_lock = sessions.lock().await;
            sessions_lock.insert(session_id.clone(), session);
            drop(sessions_lock);
            println!("[SESSION] ✓ Session saved and lock released");
            
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

/// WebSocket chat endpoint for real-time conversation
async fn websocket_chat(
    ws: WebSocketUpgrade,
    axum::extract::Extension(sessions): axum::extract::Extension<SessionManager>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, sessions))
}

/// Handle WebSocket connection for real-time chat
async fn handle_websocket(socket: WebSocket, sessions: SessionManager) {
    println!("[WEBSOCKET] New connection established");

    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));
    let mut session_id: Option<String> = None;

    // Initialize config once for this connection
    let config = match AgentConfig::from_env() {
        Ok(cfg) => {
            println!("[WEBSOCKET] Configuration loaded successfully");
            cfg
        }
        Err(e) => {
            println!("[WEBSOCKET] Configuration failed: {}", e);
            let error_msg = serde_json::json!({
                "success": false,
                "error": format!("Configuration error: {}", e),
                "session_id": null
            });
            let _ = sender.lock().await.send(Message::Text(error_msg.to_string())).await;
            return;
        }
    };

    // Create progress channel for real-time updates
    let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<String>(32);
    
    // Clone sender for the progress task
    let sender_clone = Arc::clone(&sender);
    let session_id_clone = session_id.clone();

    // Spawn task to forward progress messages to WebSocket
    tokio::spawn(async move {
        let mut progress_rx = progress_rx;
        while let Some(progress_msg) = progress_rx.recv().await {
            // Check if this is a special proof message (marked with __PROOF__)
            if progress_msg.starts_with("__PROOF__") {
                // Extract the JSON from the message and send as proofs array
                if let Ok(proof_json) = serde_json::from_str::<serde_json::Value>(&progress_msg[9..]) {
                    let proofs_msg = serde_json::json!({
                        "proofs": [proof_json],
                        "session_id": session_id_clone
                    });
                    if let Err(e) = sender_clone.lock().await.send(Message::Text(proofs_msg.to_string())).await {
                        println!("[WEBSOCKET] Error sending proof: {}", e);
                        break;
                    }
                }
            } else {
                // Regular progress message
                let progress_json = serde_json::json!({
                    "progress": progress_msg,
                    "session_id": session_id_clone
                });
                if let Err(e) = sender_clone.lock().await.send(Message::Text(progress_json.to_string())).await {
                    println!("[WEBSOCKET] Error sending progress: {}", e);
                    break;
                }
            }
        }
    });

    // Message handling loop
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                println!("[WEBSOCKET] Received message: {}", text);
                
                // Parse incoming message
                let chat_request: Result<ChatRequest, _> = serde_json::from_str(&text);
                match chat_request {
                    Ok(payload) => {
                        // SECURITY: Server generates session ID, never trusts client-provided ones
                        let session_id_for_processing = if let Some(established_session) = &session_id {
                            // Use established session ID for this connection
                            established_session.clone()
                        } else {
                            // First message - server generates new session ID (ignore client-provided)
                            let server_generated_session_id = format!("agent-a-{}", uuid::Uuid::new_v4());
                            session_id = Some(server_generated_session_id.clone());
                            println!("[WEBSOCKET] Server generated new session: {}", server_generated_session_id);
                            server_generated_session_id
                        };

                        println!("[WEBSOCKET] Processing message for session: {}", session_id_for_processing);
                        // Lock session manager and get/create session data
                        println!("[WEBSOCKET] Acquiring lock for session: {}", session_id_for_processing);
                        let mut sessions_lock = sessions.lock().await;
                        let mut session = sessions_lock
                            .get(&session_id_for_processing)
                            .cloned()
                            .unwrap_or_default();

                        println!("[WEBSOCKET] ✓ Lock acquired. Retrieved session - State: {}, Messages: {}",
                                 session.state.step, session.messages.len());
                        
                        // IMPORTANT: Release lock BEFORE processing to avoid deadlocks
                        drop(sessions_lock);
                        println!("[WEBSOCKET] Lock released before calling process_user_query");
                        
                        // Process the user query with progress channel
                        match process_user_query(&config, &payload.message, &session.messages, &mut session.state, &session_id_for_processing, Some(progress_tx.clone())).await {
                            Ok((response, updated_messages, updated_state)) => {
                                println!("[WEBSOCKET] ✓ Orchestration completed successfully");
                                // Update session with new messages and state
                                let message_count = updated_messages.len();
                                session.messages = updated_messages;
                                session.state = updated_state.clone();
                                
                                // Re-acquire lock to update session
                                println!("[WEBSOCKET] Re-acquiring lock to save updated session");
                                let mut sessions_lock = sessions.lock().await;
                                sessions_lock.insert(session_id_for_processing.clone(), session);
                                drop(sessions_lock);
                                println!("[WEBSOCKET] ✓ Session saved and lock released");
                                
                                println!("[WEBSOCKET] Request processed - SessionID: {}, New state: {}, Messages: {}",
                                         session_id_for_processing, updated_state.step, message_count);
                                
                                // Send response back through WebSocket
                                // Note: Proofs are sent in real-time via progress channel as they're submitted
                                let response_msg = serde_json::json!({
                                    "success": true,
                                    "response": response,
                                    "session_id": session_id_for_processing
                                });

                                if let Err(e) = sender.lock().await.send(Message::Text(response_msg.to_string())).await {
                                    println!("[WEBSOCKET] Error sending response: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                println!("[WEBSOCKET] Processing failed - SessionID: {}, Error: {}", session_id_for_processing, e);
                                
                                let error_msg = serde_json::json!({
                                    "success": false,
                                    "error": format!("Error processing request: {}", e),
                                    "session_id": session_id_for_processing
                                });

                                if let Err(e) = sender.lock().await.send(Message::Text(error_msg.to_string())).await {
                                    println!("[WEBSOCKET] Error sending error response: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("[WEBSOCKET] Failed to parse message: {}", e);
                        let error_msg = serde_json::json!({
                            "success": false,
                            "error": format!("Invalid message format: {}", e),
                            "session_id": session_id
                        });

                        if let Err(e) = sender.lock().await.send(Message::Text(error_msg.to_string())).await {
                            println!("[WEBSOCKET] Error sending parse error: {}", e);
                            break;
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                println!("[WEBSOCKET] Connection closed by client");
                break;
            }
            Ok(_) => {
                // Ignore other message types (ping, pong, binary, etc.)
            }
            Err(e) => {
                println!("[WEBSOCKET] WebSocket error: {}", e);
                break;
            }
        }
    }

    println!("[WEBSOCKET] Connection ended");
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
    println!("║   With Session-based Conversation & WebSocket Support      ║");
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
    
    if let Ok(attestation_url) = std::env::var("ATTESTER_URL") {
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
        .route("/ws/chat", get(websocket_chat))
        .route("/proofs/verify/:proof_id", get(get_proof_by_id))  // Most specific first
        .route("/proofs/:session_id/count", get(get_proof_count)) // Second most specific
        .route("/proofs/:session_id", get(get_proofs))             // Least specific (catch-all)
        .layer(CorsLayer::permissive())
        .layer(axum::extract::Extension(sessions));

    // Create listener
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind listener");

    println!("[STARTUP] ✓ Agent A HTTP/WebSocket Server running on http://0.0.0.0:{}", port);
    println!("  POST /chat                      — Send a message (HTTP REST API)");
    println!("  GET  /ws/chat                   — WebSocket chat endpoint (real-time)");
    println!("  POST /proofs                    — Submit a cryptographic proof");
    println!("  GET  /proofs/:session_id        — Get all proofs for session (with full ZK-TLS data)");
    println!("  GET  /proofs/:session_id/count  — Get proof count for session");
    println!("  GET  /proofs/verify/:proof_id   — Get proof by ID (optimized for on-chain verification)");
    println!("  GET  /health                    — Check server health");
    println!("\n✅ WebSocket enabled - clients can connect to ws://localhost:{}/ws/chat for real-time chat", port);
    println!("✅ Proof Verification - Any agent can independently verify proofs against Reclaim smart contract");
    println!("✅ Full proof data includes: ZK-TLS proof, request/response, workflow metadata\n");

    // Run server
    if let Err(e) = axum::serve(listener, app).await {
        println!("[FATAL] Server failed: {}", e);
    }
}
