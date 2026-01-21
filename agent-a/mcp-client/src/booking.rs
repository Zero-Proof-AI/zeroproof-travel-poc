/// Booking module
/// Handles flight booking and payment processing workflows

use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use regex::Regex;
use crate::orchestration::{AgentConfig, BookingState};
use crate::shared::{call_server_tool, call_server_tool_with_proof, submit_proof_to_database_with_progress};

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

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

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
                    
                    // Submit proof to agent-a database asynchronously with progress channel
                    let server_url = config.server_url.clone();
                    let session_id_db = session_id.to_string();
                    let crypto_proof_db = crypto_proof.clone();
                    let progress_tx_db = progress_tx.clone();
                    tokio::spawn(async move {
                        match submit_proof_to_database_with_progress(&server_url, &session_id_db, &crypto_proof_db, None, None, None, progress_tx_db).await {
                            Ok(proof_id) => {
                                println!("[PROOF] Submitted proof to agent-a database: {}", proof_id);
                            }
                            Err(e) => {
                                eprintln!("[PROOF] Failed to submit proof to agent-a database: {}", e);
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
    } else {
        // Call without proof collection
        use crate::shared::call_server_tool;
        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
            "get-ticket-price",
            price_args,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(e) => {
                eprintln!("[BOOKING] Failed to fetch pricing: {}", e);
                Err(anyhow!("Failed to fetch pricing: {}", e))
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

    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

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
    send_progress(&progress_tx, &format!("🛫 Booking flight from {} to {}...", from, to)).await;
    let book_args = json!({
        "from": from,
        "to": to,
        "passenger_name": passenger_name,
        "passenger_email": passenger_email
    });

    if enable_proof_collection {
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
                    
                    // Submit proof to agent-a database asynchronously with progress channel
                    let server_url = config.server_url.clone();
                    let session_id_db = session_id.to_string();
                    let crypto_proof_db = crypto_proof.clone();
                    let progress_tx_db = progress_tx.clone();
                    tokio::spawn(async move {
                        match submit_proof_to_database_with_progress(&server_url, &session_id_db, &crypto_proof_db, None, None, None, progress_tx_db).await {
                            Ok(proof_id) => {
                                println!("[PROOF] Submitted proof to agent-a database: {}", proof_id);
                            }
                            Err(e) => {
                                eprintln!("[PROOF] Failed to submit proof to agent-a database: {}", e);
                            }
                        }
                    });
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
            Err(e) => Err(anyhow!("Failed to complete booking: {}", e)),
        }
    } else {
        // Call without proof collection
        use crate::shared::call_server_tool;
        match call_server_tool(
            &client,
            &config.server_url,
            &agent_b_url,
            payment_agent_url,
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
            Err(e) => Err(anyhow!("Failed to complete booking: {}", e)),
        }
    }
}
