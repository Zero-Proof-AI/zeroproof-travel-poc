/// Booking module
/// Handles flight booking and payment processing workflows

use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use regex::Regex;
use crate::orchestration::{AgentConfig, BookingState};
use crate::shared::{call_server_tool, call_server_tool_with_proof, AttestationConfig, send_proof_to_ui};

/// Process error messages from server, passing through validation errors and providing fallbacks
/// 
/// If the error contains specific validation details from the server, passes them through.
/// Otherwise, provides a generic fallback with context.
fn process_tool_error(error_msg: &str, context: &str) -> String {
    if error_msg.contains("Missing required") || 
       error_msg.contains("is required") ||
       error_msg.contains("Validation") {
        // Server provided specific validation error - pass it through
        error_msg.to_string()
    } else if error_msg.contains("status 500") {
        format!("Server error while {}. Please try again later.", context)
    } else {
        error_msg.to_string()
    }
}

/// Fetch ticket pricing with optional proof collection
pub async fn get_ticket_pricing(
    config: &AgentConfig,
    session_id: &str,
    from: &str,
    to: &str,
    state: &mut BookingState,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<String> {
    let client = reqwest::Client::new();

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();
    let enable_proof_collection = std::env::var("ENABLE_PROOF_COLLECTION")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(true);

    println!("[BOOKING] Fetching ticket pricing for {} -> {} (proof_collection={})", from, to, enable_proof_collection);
    let price_args = serde_json::json!({
        "from": from,
        "to": to
    });
    
    if enable_proof_collection {
        // Create attestation config with the correct session_id
        let attestation_url = std::env::var("ATTESTER_URL")
            .unwrap_or_else(|_| "https://dev.attester.zeroproofai.com".to_string());
        let attestation_config = Some(AttestationConfig {
            service_url: attestation_url,
            enabled: true,
            workflow_stage: Some("pricing".to_string()),
            session_id: Some(session_id.to_string()),
            submitted_by: "agent-a".to_string(),
        });

        match call_server_tool_with_proof(
            &client,
            &agent_b_url,
            zkfetch_wrapper_url,
            "get-ticket-price",
            price_args,
            attestation_config.clone(),  // Pass the config with correct session_id
        )
        .await
        {
            Ok((result, proof)) => {
                // Collect cryptographic proof if available
                if let Some(crypto_proof) = proof {
                    send_proof_to_ui(crypto_proof, &attestation_config, session_id, state, &progress_tx).await;
                }
                Ok(result)
            }
            Err(e) => {
                let error_msg = e.to_string();
                eprintln!("[BOOKING] Failed to fetch pricing: {}", error_msg);
                Err(anyhow!(process_tool_error(&error_msg, "fetching pricing")))
            }
        }
    } else {
        // Call without proof collection
        use crate::shared::call_server_tool;
        match call_server_tool(
            &client,
            &agent_b_url,
            "get-ticket-price",
            price_args,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(e) => {
                let error_msg = e.to_string();
                eprintln!("[BOOKING] Failed to fetch pricing: {}", error_msg);
                Err(anyhow!(process_tool_error(&error_msg, "fetching pricing")))
            }
        }
    }
}

/// Complete a flight booking with optional proof collection
pub async fn complete_booking(
    config: &AgentConfig,
    session_id: &str,
    from: &str,
    to: &str,
    price: f64,
    passenger_name: &str,
    passenger_email: &str,
    enrollment_token_id: &str,
    instruction_id: &str,
    state: &mut BookingState,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<String> {
    let client = reqwest::Client::new();

    // Helper to send progress updates
    async fn send_progress(tx: &Option<tokio::sync::mpsc::Sender<String>>, msg: &str) {
        if let Some(sender) = tx {
            let _ = sender.send(msg.to_string()).await;
        }
    }

    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());

    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();
    let enable_proof_collection = std::env::var("ENABLE_PROOF_COLLECTION")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(true);

    let session_id = session_id.to_string();
    
    // NOTE: Proof verification is now delegated to Payment-Agent
    // Agent-A does NOT send proofId to payment tools.
    // Instead, Payment-Agent queries attestation service with sessionId to find and verify proofs.

    // Payment initiation and credential retrieval are now handled separately in orchestration
    // This function now focuses on: flight booking only

    // Complete the flight booking
    send_progress(&progress_tx, &format!("🛫 Booking flight from {} to {}...)", from, to)).await;
    let book_args = json!({
        "from": from,
        "to": to,
        "passenger_name": passenger_name,
        "passenger_email": passenger_email,
        "session_id": session_id  // Pass session_id so agent-b receives it
    });

    if enable_proof_collection {
        // Create attestation config with the correct session_id
        let attestation_url = std::env::var("ATTESTER_URL")
            .unwrap_or_else(|_| "https://dev.attester.zeroproofai.com".to_string());
        let attestation_config = Some(AttestationConfig {
            service_url: attestation_url,
            enabled: true,
            workflow_stage: Some("booking".to_string()),
            session_id: Some(session_id.to_string()),
            submitted_by: "agent-a".to_string(),
        });

        send_progress(&progress_tx, &format!("(Please wait, it takes time to generate ZKP)")).await;
        match call_server_tool_with_proof(
            &client,
            &agent_b_url,
            zkfetch_wrapper_url,
            "book-flight",
            book_args,
            attestation_config.clone(),  // Pass the config with correct session_id
        )
        .await
        {
            Ok((result, proof)) => {
                // Collect cryptographic proof if available
                if let Some(crypto_proof) = proof {
                    send_proof_to_ui(crypto_proof, &attestation_config, &session_id, state, &progress_tx).await;
                }
                
                // Use regex to extract confirmation_code from response
                let re = Regex::new(r#"\"confirmation_code\"\s*:\s*\"([^\"]+)\""#).unwrap();
                if let Some(caps) = re.captures(&result) {
                    if let Some(conf_code) = caps.get(1) {
                        return Ok(format!(
                            "🎉 Flight Booking Confirmed!\n\nConfirmation Code: {}\n\nYour flight from {} to {} has been successfully booked for {}.\n\nTotal Cost: ${:.2}\n\nA detailed confirmation email has been sent to {}.\n\nYour payment has been securely processed using biometric authentication.",
                            conf_code.as_str(), from, to, passenger_name, price, passenger_email
                        ));
                    }
                }
                Ok("Failed to book flight: confirmation code not found in response".to_string())
            }
            Err(e) => {
                let error_msg = e.to_string();
                eprintln!("[BOOKING] Failed to complete booking: {}", error_msg);
                Err(anyhow!(process_tool_error(&error_msg, "booking flight")))
            }
        }
    } else {
        // Call without proof collection
        use crate::shared::call_server_tool;
        match call_server_tool(
            &client,
            &agent_b_url,
            "book-flight",
            book_args,
        )
        .await
        {
            Ok(result) => {
                // Use regex to extract confirmation_code from response
                let re = Regex::new(r#"\"confirmation_code\"\s*:\s*\"([^\"]+)\""#).unwrap();
                if let Some(caps) = re.captures(&result) {
                    if let Some(conf_code) = caps.get(1) {
                        return Ok(format!(
                            "🎉 Flight Booking Confirmed!\n\nConfirmation Code: {}\n\nYour flight from {} to {} has been successfully booked for {}.\n\nTotal Cost: ${:.2}\n\nA detailed confirmation email has been sent to {}.\n\nYour payment has been securely processed using biometric authentication.",
                            conf_code.as_str(), from, to, passenger_name, price, passenger_email
                        ));
                    }
                }
                Ok("Failed to book flight: confirmation code not found in response".to_string())
            }
            Err(e) => {
                let error_msg = e.to_string();
                eprintln!("[BOOKING] Failed to complete booking: {}", error_msg);
                Err(anyhow!(process_tool_error(&error_msg, "booking flight")))
            }
        }
    }
}
