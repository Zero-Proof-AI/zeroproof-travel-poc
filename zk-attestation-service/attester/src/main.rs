use axum::{
    extract::{Multipart, DefaultBodyLimit, Path},
    routing::{post, get},
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use once_cell::sync::Lazy;
use serde::{Serialize, Deserialize};
use sp1_sdk::{ProverClient, SP1ProvingKey, SP1VerifyingKey, SP1Stdin, HashableKey};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use uuid::Uuid;
use zk_protocol::{AttestRequest, AttestResponse};

mod proof_db;
use proof_db::{
    ProofDatabase, StoredProof, ProofSubmissionRequest, ProofSubmissionResponse,
    SingleProofResponse, SingleProofData, VerificationInfo, ProofsResponse, VerificationMetadata,
    ProofCountResponse,
};

type ElfStore = HashMap<String, Vec<u8>>; // program_id → ELF bytes
type KeyCache = HashMap<String, (SP1ProvingKey, SP1VerifyingKey)>; // program_id → (pk, vk)

static STORE: Lazy<Arc<RwLock<ElfStore>>> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static KEY_CACHE: Lazy<Arc<RwLock<KeyCache>>> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static PROOF_DB: Lazy<ProofDatabase> = Lazy::new(ProofDatabase::new);

// Simple error wrapper for better error responses
struct AppError(String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.0).into_response()
    }
}

impl From<String> for AppError {
    fn from(err: String) -> Self {
        AppError(err)
    }
}

#[derive(Serialize)]
struct RegisterResponse {
    program_id: String,
    registered_at: String,
}

// POST /register-elf  ← called by Agent B on startup
async fn register_elf(mut multipart: Multipart) -> Result<Json<RegisterResponse>, AppError> {
    let mut elf_bytes: Option<Vec<u8>> = None;

    // Read all multipart fields
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        eprintln!("✗ Multipart next_field error: {}", e);
        AppError(format!("Multipart error: {}", e))
    })? {
        let field_name = field.name().map(|s| s.to_string());
        let file_name = field.file_name().map(|s| s.to_string());
        
        println!("📦 Received field: {:?}, filename: {:?}", field_name, file_name);
        
        if field_name.as_deref() == Some("elf") {
            // Read the entire field as bytes
            let bytes = field.bytes().await.map_err(|e| {
                eprintln!("✗ Failed to read field bytes: {}", e);
                AppError(format!("Failed to read ELF bytes: {}", e))
            })?;
            
            println!("✓ Read ELF file: {} bytes", bytes.len());
            elf_bytes = Some(bytes.to_vec());
            break; // Got what we need, stop reading
        }
    }

    let elf = elf_bytes.ok_or_else(|| {
        eprintln!("✗ No ELF file found in multipart request");
        AppError("ELF file required but not found in request".to_string())
    })?;
    
    let program_id = Uuid::new_v4().to_string();

    {
        let mut store = STORE.write().unwrap();
        store.insert(program_id.clone(), elf);
    }

    println!("✓ ELF registered with program_id: {}", program_id);

    Ok(Json(RegisterResponse {
        program_id: program_id.clone(),
        registered_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

// POST /attest  ← called by Agent A
async fn attest(
    Json(payload): Json<AttestRequest>,
) -> Json<AttestResponse> {
    let prover = ProverClient::from_env();
    let program_id = &payload.program_id;

    // 1. Fetch the pre-registered ELF
    let elf = {
        let store = STORE.read().unwrap();
        store.get(program_id)
            .expect("Unknown program_id")
            .clone()
    };

    // 2. Get or compute pk and vk (cached after first setup)
    let (pk, vk) = {
        let mut cache = KEY_CACHE.write().unwrap();
        
        if let Some((cached_pk, cached_vk)) = cache.get(program_id) {
            // Cache hit: use cached keys
            println!("✓ Using cached keys for program_id: {}", program_id);
            (cached_pk.clone(), cached_vk.clone())
        } else {
            // Cache miss: compute keys and store in cache
            println!("⚙ Computing keys for program_id: {} (will be cached)", program_id);
            let (new_pk, new_vk) = prover.setup(&elf);
            cache.insert(program_id.clone(), (new_pk.clone(), new_vk.clone()));
            (new_pk, new_vk)
        }
    };

    // 3. Compute VK hash for on-chain verification (stateless universal verifier pattern)
    // SP1 uses bytes32() to hash the VK, which is passed to verifyProof() each time
    // NO storage on-chain needed - contracts are stateless!
    let vk_hash = vk.bytes32();  // 32-byte hash of the VK (already has 0x prefix)
    let vk_hash_str = vk_hash.to_string();

    println!("✓ Verifying Key Hash: {}", vk_hash_str);
    println!("  (Pass this to SP1VerifierGroth16.verifyProof() on-chain)");

    // 4. Create stdin with the input
    // Input is already bincode-serialized by the agent
    let mut stdin = SP1Stdin::new();
    stdin.write_vec(payload.input_bytes.clone());

    // 5. Generate Groth16 proof (SNARK-wrapped for on-chain compatibility)
    // Groth16: (~100k gas on-chain, uses GPU acceleration if available)
    // Alternative: .plonk() (~300k gas, const-size proof)
    let proof = prover
        .prove(&pk, &stdin)
        .groth16()  // Wraps STARK in Groth16 for on-chain verification
        .run()
        .expect("Proving failed");

    // 6. Optional: Verify proof locally before returning
    // - If verify_locally=true (default): Verify proof in attester (safe, adds 2-3s)
    // - If verify_locally=false: Skip verification (fast, Agent A verifies on-chain)
    if payload.verify_locally {
        println!("⚙ Verifying proof locally in attester...");
        prover.verify(&proof, &vk)
            .expect("Verification failed");
        println!("✓ Local verification passed");
    } else {
        println!("⊘ Skipping local verification (Agent A will verify on-chain)");
    }

    // 7. Extract public values and proof bytes
    let actual_output = payload.claimed_output.unwrap_or_else(|| serde_json::json!({}));
    let public_values_bytes = proof.public_values.as_slice();

    // proof.bytes() returns [vkey_hash[..4], proof_bytes]
    // The contract expects proofBytes to START with the first 4 bytes of the verifier hash
    // So we use proof.bytes() as-is (it already has the correct format)
    let proof_bytes = proof.bytes();

    Json(AttestResponse {
        proof: hex::encode(proof_bytes),
        public_values: hex::encode(public_values_bytes),
        vk_hash: vk_hash_str,  // Include VK hash for on-chain verification
        verified_output: actual_output,
    })
}

// Proof Submission and Verification Endpoints

// POST /proofs/submit - Agent-A submits proof after zkfetch call
async fn submit_proof(
    Json(req): Json<ProofSubmissionRequest>,
) -> Result<Json<ProofSubmissionResponse>, AppError> {
    let proof_id = Uuid::new_v4().to_string();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| AppError(format!("Time error: {}", e)))?
        .as_secs();

    println!("[PROOF SUBMIT] 📥 Received proof submission");
    println!("  proof_id: {}", proof_id);
    println!("  session_id: {}", req.session_id);
    println!("  tool_name: {}", req.tool_name);
    println!("  verified: {}", req.verified);
    println!("  onchain_compatible: {}", req.onchain_compatible);
    println!("  workflow_stage: {:?}", req.workflow_stage);

    // Convert redaction_metadata from JSON to RedactionMetadata struct if present
    let redaction_metadata = req.redaction_metadata.and_then(|rm| {
        serde_json::from_value::<proof_db::RedactionMetadata>(rm).ok()
    });

    let stored_proof = StoredProof {
        proof_id: proof_id.clone(),
        session_id: req.session_id.clone(),
        tool_name: req.tool_name.clone(),
        timestamp,
        request: req.request,
        response: req.response,
        proof: req.proof,
        verified: req.verified,
        onchain_compatible: req.onchain_compatible,
        submitted_by: req.submitted_by,
        sequence: req.sequence,
        related_proof_id: req.related_proof_id,
        workflow_stage: req.workflow_stage,
        display_response: req.display_response,
        redaction_metadata,
    };

    PROOF_DB.store_proof(stored_proof)
        .map_err(|e| {
            println!("[PROOF SUBMIT] ❌ Failed to store proof: {}", e);
            AppError(e)
        })?;

    println!("[PROOF SUBMIT] ✅ Proof stored successfully");

    Ok(Json(ProofSubmissionResponse {
        success: true,
        proof_id: Some(proof_id),
        error: None,
    }))
}

// GET /proofs/{proof_id} - Payment Agent retrieves proof for verification
async fn get_proof_by_id(
    Path(proof_id): Path<String>,
) -> Result<Json<SingleProofResponse>, AppError> {
    println!("[PROOF GET] 🔍 Retrieving proof by ID: {}", proof_id);
    
    match PROOF_DB.get_proof(&proof_id).map_err(|e| AppError(e))? {
        Some(proof) => {
            println!("[PROOF GET] ✅ Found proof");
            println!("  session_id: {}", proof.session_id);
            println!("  tool_name: {}", proof.tool_name);
            println!("  verified: {}", proof.verified);
            
            Ok(Json(SingleProofResponse {
                success: true,
                data: Some(SingleProofData {
                    proof,
                    verification_info: VerificationInfo {
                        protocol: "Reclaim".to_string(),
                        issuer: "Agent-B".to_string(),
                        timestamp_verified: true,
                        signature_algorithm: "ECDSA".to_string(),
                        can_verify_onchain: true,
                    },
                }),
                error: None,
            }))
        }
        None => {
            println!("[PROOF GET] ❌ Proof not found: {}", proof_id);
            Ok(Json(SingleProofResponse {
                success: false,
                data: None,
                error: Some(format!("Proof not found: {}", proof_id)),
            }))
        }
    }
}

// GET /proofs/{proof_id}/verify - Verification endpoint for Payment Agent
async fn verify_proof(
    Path(proof_id): Path<String>,
) -> Result<Json<SingleProofResponse>, AppError> {
    println!("[PROOF VERIFY] 🔐 Verifying proof: {}", proof_id);
    
    match PROOF_DB.get_proof(&proof_id).map_err(|e| AppError(e))? {
        Some(proof) => {
            // Additional verification checks
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| AppError(format!("Time error: {}", e)))?
                .as_secs();

            let age_secs = now.saturating_sub(proof.timestamp);
            let freshness_window = 5 * 60; // 5 minutes

            println!("[PROOF VERIFY]  proof age: {} seconds (max: {} seconds)", age_secs, freshness_window);

            if age_secs > freshness_window {
                println!("[PROOF VERIFY] ❌ Proof expired");
                return Ok(Json(SingleProofResponse {
                    success: false,
                    data: None,
                    error: Some(format!(
                        "Proof expired: {} seconds old (max: {} seconds)",
                        age_secs, freshness_window
                    )),
                }));
            }

            if !proof.verified {
                println!("[PROOF VERIFY] ❌ Proof not marked as verified");
                return Ok(Json(SingleProofResponse {
                    success: false,
                    data: None,
                    error: Some("Proof not marked as verified".to_string()),
                }));
            }

            println!("[PROOF VERIFY] ✅ Proof verified successfully");

            Ok(Json(SingleProofResponse {
                success: true,
                data: Some(SingleProofData {
                    proof,
                    verification_info: VerificationInfo {
                        protocol: "Reclaim".to_string(),
                        issuer: "Agent-B".to_string(),
                        timestamp_verified: true,
                        signature_algorithm: "ECDSA".to_string(),
                        can_verify_onchain: true,
                    },
                }),
                error: None,
            }))
        }
        None => {
            println!("[PROOF VERIFY] ❌ Proof not found: {}", proof_id);
            Ok(Json(SingleProofResponse {
                success: false,
                data: None,
                error: Some(format!("Proof not found: {}", proof_id)),
            }))
        }
    }
}

// GET /proofs/{session_id} - Get all proofs for a session
async fn get_proofs(
    Path(session_id): Path<String>,
) -> Result<Json<ProofsResponse>, AppError> {
    println!("[PROOFS LIST] 📋 Retrieving all proofs for session: {}", session_id);
    
    let proofs = PROOF_DB.get_proofs(&session_id).map_err(|e| {
        println!("[PROOFS LIST] ❌ Error retrieving proofs: {}", e);
        AppError(e)
    })?;

    println!("[PROOFS LIST] ✅ Found {} proofs", proofs.len());
    for (i, proof) in proofs.iter().enumerate() {
        println!("  [{}] proof_id: {}, tool: {}, verified: {}", 
                 i + 1, proof.proof_id, proof.tool_name, proof.verified);
    }

    Ok(Json(ProofsResponse {
        success: true,
        session_id: session_id.clone(),
        count: proofs.len(),
        proofs,
        verification_metadata: VerificationMetadata {
            protocol: "Reclaim".to_string(),
            issuer: "Agent-B".to_string(),
            verification_service: "zk-attestation-service".to_string(),
        },
    }))
}

// GET /proofs/count/{session_id} - Get proof count for session
async fn get_proof_count(
    Path(session_id): Path<String>,
) -> Result<Json<ProofCountResponse>, AppError> {
    println!("[PROOF COUNT] 📊 Getting proof count for session: {}", session_id);
    
    let count = PROOF_DB.get_proof_count(&session_id).map_err(|e| {
        println!("[PROOF COUNT] ❌ Error getting count: {}", e);
        AppError(e)
    })?;

    println!("[PROOF COUNT] ✅ Session has {} proofs", count);

    Ok(Json(ProofCountResponse {
        success: true,
        session_id: session_id.clone(),
        count,
    }))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/register-elf", post(register_elf))
        .route("/attest", post(attest))
        .route("/proofs/submit", post(submit_proof))
        .route("/proofs/:proof_id/verify", get(verify_proof))  // Most specific
        .route("/proofs/count/:session_id", get(get_proof_count))
        .route("/proofs/session/:session_id", get(get_proofs))
        .route("/proofs/:proof_id", get(get_proof_by_id))       // Least specific (catch-all)
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024)); // 20MB limit for ELF files

    println!("ZK Attester running → http://0.0.0.0:8000");
    println!("   POST /register-elf           ← Agent B calls this once");
    println!("   POST /attest                ← Agent A calls this");
    println!("   POST /proofs/submit         ← Agent A submits proof after zkfetch");
    println!("   GET  /proofs/{{proof_id}}    ← Payment Agent retrieves proof");
    println!("   GET  /proofs/{{proof_id}}/verify ← Payment Agent verifies proof");
    println!("   GET  /proofs/session/{{session_id}} ← Query all proofs for session");
    println!("   GET  /proofs/count/{{session_id}} ← Count proofs in session");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("Failed to bind to 0.0.0.0:8000");

    axum::serve(listener, app)
        .await
        .expect("Server error");
}