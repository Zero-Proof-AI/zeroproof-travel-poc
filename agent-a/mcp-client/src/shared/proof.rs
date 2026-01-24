/// Proof submission and storage module
/// Handles cryptographic proof submission to attestation services and databases

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Collect and send cryptographic proof to UI via progress channel
/// 
/// This function:
/// - Pushes the proof to BookingState's cryptographic_traces
/// - Builds a JSON message with all metadata
/// - Adds workflow_stage and submitted_by from attestation config
/// - Sends it to the UI via the progress channel with __PROOF__ prefix
pub async fn send_proof_to_ui(
    crypto_proof: CryptographicProof,
    attestation_config: &Option<crate::shared::AttestationConfig>,
    session_id: &str,
    state: &mut crate::orchestration::BookingState,
    progress_tx: &Option<tokio::sync::mpsc::Sender<String>>,
) {
    state.cryptographic_traces.push(crypto_proof.clone());
    println!("[PROOF] Collected proof for {}: {}", crypto_proof.tool_name, state.cryptographic_traces.len());
    
    // Send proof to UI via progress channel with all available metadata
    if let Some(tx) = progress_tx {
        let mut proof_msg = json!({
            "tool_name": crypto_proof.tool_name,
            "timestamp": crypto_proof.timestamp,
            "verified": crypto_proof.verified,
            "onchain_compatible": crypto_proof.onchain_compatible,
            "proof_id": format!("{}_{}", session_id, crypto_proof.timestamp),
            "request": crypto_proof.request,
            "response": crypto_proof.response,
            "proof": crypto_proof.proof,
            "session_id": session_id,
        });
        
        // Add workflow_stage and submitted_by from attestation config
        if let Some(config_ref) = attestation_config {
            if let Some(stage) = &config_ref.workflow_stage {
                proof_msg["workflow_stage"] = json!(stage);
            }
            proof_msg["submitted_by"] = json!(&config_ref.submitted_by);
        }
        
        let _ = tx.send(format!("__PROOF__{}", proof_msg.to_string())).await;
    }
    // Proof submission is now handled automatically by ProxyFetch via attestation_config
}

/// Metadata tracking which fields were redacted from a proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionMetadata {
    /// Number of fields that were redacted
    pub redacted_field_count: usize,
    /// List of dot-notation paths that were redacted
    pub redacted_paths: Vec<String>,
    /// Whether redactions were applied (true if any fields were redacted)
    pub was_redacted: bool,
}

impl Default for RedactionMetadata {
    fn default() -> Self {
        Self {
            redacted_field_count: 0,
            redacted_paths: Vec::new(),
            was_redacted: false,
        }
    }
}

/// Cryptographic proof record for tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptographicProof {
    pub tool_name: String,
    pub timestamp: u64,
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub proof: serde_json::Value, // zkfetch proof
    pub proof_id: Option<String>,
    pub verified: bool,
    pub onchain_compatible: bool,
    
    /// Display version of response with sensitive fields redacted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_response: Option<serde_json::Value>,
    
    /// Metadata about which fields were redacted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_metadata: Option<RedactionMetadata>,
}

/// Submit a proof to zk-attestation-service for independent verification
pub async fn submit_proof(
    client: &reqwest::Client,
    attestation_service_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
    workflow_stage: Option<String>,
    submitted_by: &str,
) -> Result<String> {
    let submit_url = format!("{}/proofs/submit", attestation_service_url);
    
    let payload = json!({
        "session_id": session_id,
        "tool_name": proof.tool_name,
        "timestamp": proof.timestamp,
        "request": proof.request,
        "response": proof.response,
        "proof": proof.proof,
        "verified": proof.verified,
        "onchain_compatible": proof.onchain_compatible,
        "submitted_by": submitted_by,
        "workflow_stage": workflow_stage.or_else(|| Some("general".to_string())),
        "display_response": proof.display_response,
        "redaction_metadata": proof.redaction_metadata,
    });
    
    let response = match client
        .post(&submit_url)
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("[PROOF] Error sending request to {}: {:?}", submit_url, e);
            eprintln!("[PROOF] Error details: {}", e.to_string());
            if let Some(status) = e.status() {
                eprintln!("[PROOF] HTTP Status: {}", status);
            }
            if e.is_connect() {
                eprintln!("[PROOF] Connection error - is the attestation service running at {}?", attestation_service_url);
            }
            if e.is_timeout() {
                eprintln!("[PROOF] Request timeout");
            }
            return Err(anyhow!("Failed to send request to attestation service: {}", e));
        }
    };
    
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_else(|_| "<could not read response body>".to_string());
        eprintln!("[PROOF] Attestation service returned error status: {}", status);
        eprintln!("[PROOF] Response body: {}", error_text);
        return Err(anyhow!("Failed to submit proof: HTTP {} - {}", status, error_text));
    }
    
    let result: Value = response.json().await?;
    
    if let Some(proof_id) = result.get("proof_id").and_then(|p| p.as_str()) {
        println!("[PROOF] ✓ Proof submitted to attestation service: {}", proof_id);
        Ok(proof_id.to_string())
    } else {
        Err(anyhow!("No proof_id in response from attestation service"))
    }
}
