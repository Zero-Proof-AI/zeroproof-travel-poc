/// Proof submission and storage module
/// Handles cryptographic proof submission to attestation services and databases

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
pub async fn submit_proof_to_attestation_service(
    client: &reqwest::Client,
    attestation_service_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
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
        "submitted_by": "agent-a",
        "workflow_stage": "pricing",
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
        
        // Send proof to UI via WebSocket if progress channel is available
        if let Some(tx) = progress_tx {
            let proof_msg = json!({
                "tool_name": proof.tool_name,
                "proof_id": proof_id,
                "timestamp": proof.timestamp,
                "verified": proof.verified,
                "onchain_compatible": proof.onchain_compatible,
            });
            let _ = tx.send(format!("__PROOF__{}", proof_msg.to_string())).await;
            println!("[PROOF] ✓ Proof sent to UI via WebSocket");
        }
        
        Ok(proof_id.to_string())
    } else {
        Err(anyhow!("No proof_id in response from attestation service"))
    }
}

/// Submit a cryptographic proof to the Agent-A proof database with workflow metadata
pub async fn submit_proof_to_database(
    server_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
) -> Result<String> {
    submit_proof_to_database_with_progress(
        server_url,
        session_id,
        proof,
        None,  // sequence - will be auto-assigned by database
        None,  // related_proof_id
        None,  // workflow_stage - will be inferred from tool_name
        None,  // progress_tx
    ).await
}

/// Submit a proof with full workflow metadata and optional progress channel
pub async fn submit_proof_to_database_with_progress(
    server_url: &str,
    session_id: &str,
    proof: &CryptographicProof,
    sequence: Option<u32>,
    related_proof_id: Option<String>,
    workflow_stage: Option<String>,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("{}/proofs", server_url);
    
    // Infer workflow_stage from tool_name if not provided
    let inferred_stage = workflow_stage.or_else(|| {
        match proof.tool_name.as_str() {
            "get-ticket-price" | "get-flight-options" => Some("pricing".to_string()),
            "enroll-card" => Some("payment_enrollment".to_string()),
            "create-payment-instruction" | "pay-for-ticket" => Some("payment".to_string()),
            "book-flight" => Some("booking".to_string()),
            _ => None,
        }
    });
    
    let mut payload = json!({
        "session_id": session_id,
        "tool_name": proof.tool_name,
        "timestamp": proof.timestamp,
        "request": proof.request,
        "response": proof.response,
        "proof": proof.proof,
        "proof_id": proof.proof_id,
        "verified": proof.verified,
        "onchain_compatible": proof.onchain_compatible,
        "submitted_by": "agent-a"
    });
    
    // Add optional workflow metadata
    if let Some(seq) = sequence {
        payload["sequence"] = json!(seq);
    }
    if let Some(ref rel_id) = related_proof_id {
        payload["related_proof_id"] = json!(rel_id);
    }
    if let Some(ref stage) = inferred_stage {
        payload["workflow_stage"] = json!(stage);
    }
    
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow!("Failed to submit proof: {}", error_text));
    }
    
    let result: Value = response.json().await?;
    
    if let Some(proof_id) = result.get("proof_id").and_then(|p| p.as_str()) {
        println!("[PROOF] ✓ Proof submitted to database: {}", proof_id);
        
        // Send proof to UI via WebSocket if progress channel is available
        if let Some(tx) = progress_tx {
            let proof_msg = json!({
                "tool_name": proof.tool_name,
                "proof_id": proof_id,
                "timestamp": proof.timestamp,
                "verified": proof.verified,
                "onchain_compatible": proof.onchain_compatible,
            });
            let _ = tx.send(format!("__PROOF__{}", proof_msg.to_string())).await;
            println!("[PROOF] ✓ Proof sent to UI via WebSocket");
        }
        
        Ok(proof_id.to_string())
    } else {
        Err(anyhow!("Invalid proof submission response"))
    }
}
