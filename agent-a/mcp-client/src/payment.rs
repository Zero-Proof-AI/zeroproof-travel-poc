/// Payment module
/// Handles payment-related operations: enrollment, payment initiation, credential retrieval

use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use crate::orchestration::{AgentConfig, BookingState};
use crate::shared::{call_server_tool, call_server_tool_with_proof, AttestationConfig};
use regex::Regex;

/// Parse and verify enroll-card response
/// Handles redacted responses using regex extraction
/// 
/// Full response WITHOUT redaction:
/// {
///   "correlationId": "a03623bd-fdac-44bf-a225-72e28a9804dd",
///   "data": {
///     "biometricEnabled": true,
///     "clientReferenceId": "visa_ref_e04109cb-ca86-48bd-95f0-707663f75768",
///     "enrollmentStatus": "ACTIVE",
///     "status": "SUCCESS",
///     "tokenId": "token_45352b41-e4f3-4ce3-86c4-24fbf41adecf"
///   },
///   "success": true
/// }
///
/// Full response WITH redaction (double-encoded string):
/// "\"tokenId\":\"token_a2521735-4873-44be-aae8-3c38cdf5d76f\""
fn parse_enroll_card_response(result: &str) -> Result<String> {
    println!("[PAYMENT] Parsing enroll-card response: {}", result);
    
    // Use regex to extract tokenId from response
    // Matches: "tokenId":"value", \"tokenId\":\"value\", or just tokenId:value
    // The pattern handles escaped quotes: \\\" and regular quotes: "
    let token_pattern = Regex::new(r#"(?:\\?["\'])?tokenId(?:\\?["\'])?\s*:\s*(?:\\?["\'])([^"\'\\]+)(?:\\?["\'])"#)
        .map_err(|e| anyhow!("Regex error: {}", e))?;
    
    if let Some(caps) = token_pattern.captures(result) {
        if let Some(token_id) = caps.get(1) {
            let extracted_token = token_id.as_str().to_string();
            println!("[PAYMENT] ✓ Card enrollment succeeded: token_id={}", extracted_token);
            return Ok(extracted_token);
        }
    }
    
    println!("[PAYMENT] ✗ Card enrollment failed. Could not extract tokenId from response: {}", result);
    Err(anyhow!("Card enrollment failed: tokenId not found in response"))
}

/// Step 2: Enroll card if needed
/// Returns (enrollment_token_id, enrollment_complete)
pub async fn enroll_card_if_needed(
    config: &AgentConfig,
    session_id: &str,
    payment_agent_url: Option<&str>,
    state: &mut BookingState,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<(String, bool)> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    let mut enrollment_token_id = "token_789".to_string();
    let mut enrollment_complete = false;

    // Check if card is already enrolled
    if let Some(payment_url) = payment_agent_url {
        let session_url = format!("{}/session/{}", payment_url, session_id);

        if let Ok(response) = client.get(&session_url).send().await {
            if let Ok(session_data) = response.json::<Value>().await {
                if let Some(data) = session_data.get("data") {
                    if let Some(token_count) = data.get("enrolledTokenCount").and_then(|c| c.as_u64()) {
                        if token_count > 0 {
                            enrollment_complete = true;
                            if let Some(token_ids) = data.get("enrolledTokenIds").and_then(|ids| ids.as_array()) {
                                if let Some(first_token) = token_ids.first().and_then(|t| t.as_str()) {
                                    enrollment_token_id = first_token.to_string();
                                }
                            }
                            return Ok((enrollment_token_id, enrollment_complete));
                        }
                    }
                }
            }
        }
    }

    // Card not enrolled yet, need to enroll
    if !enrollment_complete && payment_agent_url.is_some() {
        let enroll_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "enrollmentReferenceId": "enroll_ref_456"
        });

        // NOTE: proofId NOT sent here. Payment-Agent queries attestation service
        // with sessionId to find and verify proofs autonomously.
        // This prevents Agent-A from dictating which proof to verify.

        // Create attestation config with the correct session_id
        let attestation_config = Some(AttestationConfig {
            service_url: "https://dev.attester.zeroproofai.com".to_string(),
            enabled: true,
            workflow_stage: Some("payment_enrollment".to_string()),
            session_id: Some(session_id.to_string()),
            submitted_by: "agent-a".to_string(),
        });

        match call_server_tool_with_proof(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            zkfetch_wrapper_url,
            "enroll-card",
            enroll_args,
            attestation_config.clone(),  // Pass the config with correct session_id
        )
        .await
        {
            Ok((result, proof)) => {
                // Collect cryptographic proof if available
                if let Some(crypto_proof) = proof {
                    state.cryptographic_traces.push(crypto_proof.clone());
                    println!("[PROOF] Collected proof for enroll-card: {}", state.cryptographic_traces.len());
                    
                    // Send proof to UI via progress channel with all available metadata
                    if let Some(tx) = &progress_tx {
                        let mut proof_msg = serde_json::json!({
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
                        if let Some(config_ref) = &attestation_config {
                            if let Some(stage) = &config_ref.workflow_stage {
                                proof_msg["workflow_stage"] = serde_json::json!(stage);
                            }
                            proof_msg["submitted_by"] = serde_json::json!(&config_ref.submitted_by);
                        }
                        
                        let _ = tx.send(format!("__PROOF__{}", proof_msg.to_string())).await;
                    }
                    // Proof submission is now handled automatically by ProxyFetch via attestation_config
                }

                match parse_enroll_card_response(&result) {
                    Ok(token_id) => {
                        enrollment_token_id = token_id;
                        enrollment_complete = true;
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => {
                return Err(anyhow!("Card enrollment error: {}", e));
            }
        }
    }

    Ok((enrollment_token_id, enrollment_complete))
}

/// Step 3: Initiate payment
/// Returns instruction_id
pub async fn initiate_payment(
    config: &AgentConfig,
    session_id: &str,
    enrollment_token_id: &str,
    price: f64,
    payment_agent_url: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let mut instruction_id = String::new();

    if payment_agent_url.is_some() {
        let purchase_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "tokenId": enrollment_token_id,
            "amount": price.to_string(),
            "merchant": "ZeroProof Travel"
        });

        // NOTE: proofId NOT sent here. Payment-Agent queries attestation service
        // with sessionId to find pricing proofs and verify amount matches.
        // Payment-Agent is responsible for proof selection and verification.

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "initiate-purchase-instruction",
            purchase_args,
        )
        .await
        {
            Ok(result) => {
                if let Ok(purchase_response) = serde_json::from_str::<Value>(&result) {
                    if let Some(id) = purchase_response
                        .get("data")
                        .and_then(|data| data.get("instructionId"))
                        .or_else(|| purchase_response.get("instructionId"))
                        .and_then(|id| id.as_str())
                    {
                        instruction_id = id.to_string();
                    } else {
                        return Err(anyhow!("Could not extract instructionId from payment response"));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("Payment initiation error: {}", e));
            }
        }
    }

    Ok(instruction_id)
}

/// Step 4: Retrieve payment credentials
pub async fn retrieve_payment_credentials(
    config: &AgentConfig,
    session_id: &str,
    enrollment_token_id: &str,
    instruction_id: &str,
    payment_agent_url: Option<&str>,
) -> Result<()> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    if !instruction_id.is_empty() && payment_agent_url.is_some() {
        let retrieve_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "tokenId": enrollment_token_id,
            "instructionId": instruction_id,
            "transactionReferenceId": "txn_202"
        });

        // NOTE: proofId NOT sent here. Payment-Agent has already queried and verified
        // proofs during earlier payment steps (enroll-card, initiate-purchase-instruction).
        // This step retrieves the final credentials based on verified payment state.

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "retrieve-payment-credentials",
            retrieve_args,
        )
        .await
        {
            Ok(_result) => {
                // Payment confirmed
                Ok(())
            }
            Err(e) => {
                Err(anyhow!("Payment credential retrieval error: {}", e))
            }
        }
    } else {
        Ok(())
    }
}
