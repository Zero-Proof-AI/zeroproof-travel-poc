/// Booking module
/// Handles flight booking and payment processing workflows

use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use crate::orchestration::{AgentConfig, BookingState};
use crate::shared::{call_server_tool, call_server_tool_with_proof, submit_proof_to_attestation_service, submit_proof_to_database};

/// Fetch ticket pricing with proof collection
pub async fn get_ticket_pricing(
    config: &AgentConfig,
    session_id: &str,
    from: &str,
    to: &str,
    state: &mut BookingState,
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    println!("[BOOKING] Fetching ticket pricing for {} -> {}", from, to);
    let price_args = serde_json::json!({
        "from": from,
        "to": to
    });
    
    match call_server_tool_with_proof(
        &client,
        &config.server_url,
        &agent_b_url,
        payment_agent_url,
        zkfetch_wrapper_url,
        "get-ticket-price",
        price_args,
    )
    .await
    {
        Ok((result, proof)) => {
            // Collect and submit cryptographic proof if available
            if let Some(crypto_proof) = proof {
                state.cryptographic_traces.push(crypto_proof.clone());
                println!("[PROOF] Collected proof for get-ticket-price: {}", state.cryptographic_traces.len());
                
                // Submit proof to agent-a database asynchronously
                let server_url = config.server_url.clone();
                let session_id_db = session_id.to_string();
                let crypto_proof_db = crypto_proof.clone();
                tokio::spawn(async move {
                    match submit_proof_to_database(&server_url, &session_id_db, &crypto_proof_db).await {
                        Ok(proof_id) => {
                            println!("[PROOF] Submitted proof to agent-a database: {}", proof_id);
                        }
                        Err(e) => {
                            eprintln!("[PROOF] Failed to submit proof to agent-a database: {}", e);
                        }
                    }
                });
                
                // Submit proof to zk-attestation-service for independent verification
                let attestation_url = std::env::var("ATTESTATION_SERVICE_URL")
                    .unwrap_or_else(|_| "http://localhost:8000".to_string());
                let session_id_attest = session_id.to_string();
                let client_attest = reqwest::Client::new();
                let crypto_proof_attest = crypto_proof.clone();
                
                tokio::spawn(async move {
                    match submit_proof_to_attestation_service(
                        &client_attest,
                        &attestation_url,
                        &session_id_attest,
                        &crypto_proof_attest
                    ).await {
                        Ok(proof_id) => {
                            println!("[PROOF] ✓ Proof submitted to attestation service for {}: {}", crypto_proof_attest.tool_name, proof_id);
                        }
                        Err(e) => {
                            eprintln!("[PROOF] ✗ Failed to submit proof to attestation service for {}: {}", crypto_proof_attest.tool_name, e);
                        }
                    }
                });
            }
            Ok(result)
        }
        Err(e) => {
            eprintln!("[BOOKING] Failed to fetch pricing: {}", e);
            Err(anyhow!("Failed to fetch pricing: {}", e))
        }
    }
}

/// Complete a flight booking with payment processing
pub async fn complete_booking_with_payment(
    config: &AgentConfig,
    session_id: &str,
    from: &str,
    to: &str,
    price: f64,
    passenger_name: &str,
    passenger_email: &str,
    state: &mut BookingState,
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    let session_id = session_id.to_string();
    
    let mut enrollment_token_id = "token_789".to_string();
    let mut enrollment_complete = false;
    
    // NOTE: Proof verification is now delegated to Payment-Agent
    // Agent-A does NOT send proofId to payment tools.
    // Instead, Payment-Agent queries attestation service with sessionId to find and verify proofs.

    // Step 1: Check if card is already enrolled (in complete_booking_with_payment)
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
                        }
                    }
                }
            }
        }
    }

    // Step 2: Enroll card if needed
    if !enrollment_complete && payment_agent_url.is_some() {
        let enroll_args = json!({
            "sessionId": session_id,
            "consumerId": "user_123",
            "enrollmentReferenceId": "enroll_ref_456"
        });
        
        // NOTE: proofId NOT sent here. Payment-Agent queries attestation service
        // with sessionId to find and verify proofs autonomously.
        // This prevents Agent-A from dictating which proof to verify.

        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "enroll-card",
            enroll_args,
        )
        .await
        {
            Ok(result) => {
                if let Ok(parsed) = serde_json::from_str::<Value>(&result) {
                    let is_success = parsed.get("success").and_then(|s| s.as_bool()).unwrap_or(false) ||
                        parsed.get("status").and_then(|s| s.as_str()).map(|s| s == "SUCCESS").unwrap_or(false);
                    
                    if is_success {
                        let token_id = parsed
                            .get("data")
                            .and_then(|data| data.get("tokenId"))
                            .or_else(|| parsed.get("tokenId"))
                            .and_then(|t| t.as_str());
                        
                        if let Some(token_id) = token_id {
                            enrollment_token_id = token_id.to_string();
                        }
                        enrollment_complete = true;
                    } else {
                        return Err(anyhow!("Card enrollment failed"));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("Card enrollment error: {}", e));
            }
        }
    }

    // Step 3: Initiate payment
    let mut instruction_id = String::new();
    if enrollment_complete && payment_agent_url.is_some() {
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

    // Step 4: Retrieve payment credentials
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
                // Payment confirmed, continue to booking
            }
            Err(e) => {
                return Err(anyhow!("Payment credential retrieval error: {}", e));
            }
        }
    }

    // Step 5: Complete the flight booking
    let book_args = json!({
        "from": from,
        "to": to,
        "passenger_name": passenger_name,
        "passenger_email": passenger_email
    });

    match call_server_tool_with_proof(
        &client,
        &config.server_url,
        &agent_b_url,
        payment_agent_url,
        zkfetch_wrapper_url,
        "book-flight",
        book_args,
    )
    .await
    {
        Ok((result, proof)) => {
            // Collect and submit cryptographic proof if available
            if let Some(crypto_proof) = proof {
                state.cryptographic_traces.push(crypto_proof.clone());
                println!("[PROOF] Collected proof for book-flight: {}", state.cryptographic_traces.len());
                
                // Submit proof to agent-a database asynchronously
                let server_url = config.server_url.clone();
                let session_id_db = session_id.to_string();
                let crypto_proof_db = crypto_proof.clone();
                tokio::spawn(async move {
                    match submit_proof_to_database(&server_url, &session_id_db, &crypto_proof_db).await {
                        Ok(proof_id) => {
                            println!("[PROOF] Submitted proof to agent-a database: {}", proof_id);
                        }
                        Err(e) => {
                            eprintln!("[PROOF] Failed to submit proof to agent-a database: {}", e);
                        }
                    }
                });
                
                // Submit proof to zk-attestation-service for independent verification
                let attestation_url = std::env::var("ATTESTATION_SERVICE_URL")
                    .unwrap_or_else(|_| "http://localhost:8000".to_string());
                let session_id_attest = session_id.to_string();
                let client_attest = reqwest::Client::new();
                let crypto_proof_attest = crypto_proof.clone();
                
                tokio::spawn(async move {
                    match submit_proof_to_attestation_service(
                        &client_attest,
                        &attestation_url,
                        &session_id_attest,
                        &crypto_proof_attest
                    ).await {
                        Ok(proof_id) => {
                            println!("[PROOF] ✓ Proof submitted to attestation service for {}: {}", crypto_proof_attest.tool_name, proof_id);
                        }
                        Err(e) => {
                            eprintln!("[PROOF] ✗ Failed to submit proof to attestation service for {}: {}", crypto_proof_attest.tool_name, e);
                        }
                    }
                });
            }
            
            if let Ok(booking) = serde_json::from_str::<Value>(&result) {
                if let Some(conf_code) = booking.get("confirmation_code").and_then(|c| c.as_str()) {
                    return Ok(format!(
                        "🎉 Flight Booking Confirmed!\n\nConfirmation Code: {}\n\nYour flight from {} to {} has been successfully booked for {}.\n\nTotal Cost: ${:.2}\n\nA detailed confirmation email has been sent to {}.\n\nYour payment has been securely processed using biometric authentication.",
                        conf_code, from, to, passenger_name, price, passenger_email
                    ));
                }
            }
            Ok(format!("Booking completed. Result: {}", result))
        }
        Err(e) => Err(anyhow!("Failed to complete booking: {}", e)),
    }
}
