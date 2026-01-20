/// Orchestration logic for Agent A - extracted from main.rs for reuse
/// This module contains all the core agent logic:
/// - Claude API calls
/// - Tool routing and execution
/// - Payment workflows
/// - Proxy-fetch integration

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::shared::{fetch_all_tools, parse_tool_calls, call_claude, call_server_tool, call_server_tool_with_proof, submit_proof_to_attestation_service, submit_proof_to_database, CryptographicProof};
use crate::prompts::extract_with_claude;
use crate::booking::complete_booking_with_payment;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: String,
}

/// Booking state tracking across multi-turn conversations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingState {
    pub step: String, // "initial", "pricing", "passenger_name", "passenger_email", "payment_method", "enrollment_confirmation", "payment", "completed"
    pub from: String,
    pub to: String,
    pub price: f64,
    pub passenger_name: Option<String>,
    pub passenger_email: Option<String>,
    pub payment_method: Option<String>, // "visa", "other", etc.
    pub enrollment_token_id: Option<String>,
    pub instruction_id: Option<String>,
    pub vip: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cryptographic_traces: Vec<CryptographicProof>, // Collected proofs from agent-b calls - single source of truth for all proofs
}

impl Default for BookingState {
    fn default() -> Self {
        Self {
            step: "initial".to_string(),
            from: String::new(),
            to: String::new(),
            price: 0.0,
            passenger_name: None,
            passenger_email: None,
            payment_method: None,
            enrollment_token_id: None,
            instruction_id: None,
            vip: false,
            cryptographic_traces: Vec::new(),
        }
    }
}

/// Agent configuration
pub struct AgentConfig {
    pub claude_api_key: String,
    pub server_url: String,
    pub payment_agent_url: Option<String>,
    pub payment_agent_enabled: bool,
    pub zkfetch_wrapper_url: Option<String>,
}

impl AgentConfig {
    pub fn from_env() -> Result<Self> {
        let claude_api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow!("ANTHROPIC_API_KEY environment variable not set"))?;
        
        let server_url = std::env::var("AGENT_A_SERVER_URL")
            .unwrap_or_else(|_| "http://localhost:3001".to_string());
        
        let payment_agent_url = std::env::var("PAYMENT_AGENT_URL").ok();
        let payment_agent_enabled = std::env::var("PAYMENT_AGENT_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase() == "true";
        
        let zkfetch_wrapper_url = std::env::var("ZKFETCH_WRAPPER_URL").ok();

        Ok(Self {
            claude_api_key,
            server_url,
            payment_agent_url,
            payment_agent_enabled,
            zkfetch_wrapper_url,
        })
    }
}

/// Process a user query through the full orchestration pipeline
/// Handles multi-turn conversations including booking workflows
/// Returns (response_text, updated_messages, updated_state)
pub async fn process_user_query(
    config: &AgentConfig,
    user_query: &str,
    messages: &[ClaudeMessage],
    state: &mut BookingState,
    session_id: &str,
) -> Result<(String, Vec<ClaudeMessage>, BookingState)> {
    let client = reqwest::Client::new();

    // Fetch tool definitions
    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());
    
    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };
    
    let zkfetch_wrapper_url = config.zkfetch_wrapper_url.as_deref();

    let tool_definitions = fetch_all_tools(&client, &config.server_url, &agent_b_url, payment_agent_url).await?;

    // Call Claude with full message history
    let claude_response = call_claude(&client, config, user_query, messages, state, &tool_definitions, None).await?;
    // Build updated message list
    let mut updated_messages = messages.to_vec();
    
    // Parse tool calls
    match parse_tool_calls(&claude_response) {
        Ok(tool_calls) => {
            println!("[PARSE] Successfully parsed {} tool calls", tool_calls.len());
            for (name, _) in &tool_calls {
                println!("[PARSE] - Tool: {}", name);
            }
            
            if tool_calls.is_empty() {
                // No tools needed, return Claude's response
                updated_messages.push(ClaudeMessage {
                    role: "assistant".to_string(),
                    content: claude_response.clone(),
                });
                
                // Extract name and email using Claude's understanding
                // User provided name, now ask for email
                if state.step == "passenger_name" && state.passenger_name.is_none() {
                    println!("[DEBUG] Processing passenger_name step");
                    let extracted_name = extract_with_claude(&client, config, "passenger_name", user_query, state, &tool_definitions).await?;
                    println!("[DEBUG] Extracted name result: '{}' (empty: {})", extracted_name, extracted_name.is_empty());
                    
                    if !extracted_name.is_empty() {
                        state.passenger_name = Some(extracted_name.clone());
                        state.step = "passenger_email".to_string();
                        println!("[DEBUG] State updated to: {}", state.step);
                        let response = format!(
                            "Agent A: Perfect! Got it - {}.\n\n📧 Step 2: Email Address\n\nWhat is your email address?",
                            extracted_name
                        );
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    } else {
                        // Couldn't extract name, ask again
                        let response = "Agent A: I couldn't understand that. Could you please provide your full name?".to_string();
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    }
                }
                
                // User provided email, now ask for payment method
                if state.step == "passenger_email" && state.passenger_email.is_none() {
                    let extracted_email = extract_with_claude(&client, config, "passenger_email", user_query, state, &tool_definitions).await?;
                    
                    if !extracted_email.is_empty() {
                        state.passenger_email = Some(extracted_email.clone());
                        state.step = "payment_method".to_string();
                        let passenger_name = state.passenger_name.clone().unwrap_or_default();
                        
                        let response = format!(
                            "Agent A: Excellent! I have your details:\n- Name: {}\n- Email: {}\n\n💳 Step 3: Payment Method\n\nHow would you like to pay for this ${} flight?\n1. Visa Credit Card\n2. Other payment method\n\nPlease reply with 1 or 2.",
                            passenger_name, extracted_email, state.price as i32
                        );
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    } else {
                        // Couldn't extract email, ask again
                        let response = "Agent A: I couldn't find a valid email. Please provide your email address (e.g., user@example.com):".to_string();
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    }
                }
                
                // User selected payment method. Ask for enrollment confirmation.
                if state.step == "payment_method" {
                    let payment_method = extract_with_claude(&client, config, "payment_method", user_query, state, &tool_definitions).await?;
                    
                    // Check if user actually responded to payment method question
                    if !payment_method.contains("1") && !payment_method.contains("2") 
                        && !payment_method.contains("visa") && !payment_method.contains("other")
                        && !payment_method.contains("credit") && !payment_method.contains("card") {
                        // User didn't answer the payment method question clearly
                        let response = "Agent A: I need you to select your payment method. Please reply with:\n1. Visa Credit Card\n2. Other payment method".to_string();
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    }
                    
                    let selected_method = if payment_method.contains("1") || payment_method.contains("visa") {
                        "Visa Credit Card"
                    } else {
                        "Visa Credit Card" // Default to Visa if other selected
                    };
                    
                    // Update state with payment method
                    state.step = "enrollment_confirmation".to_string();
                    state.payment_method = Some(selected_method.to_string());
                    
                    let response = format!(
                        "Agent A: Perfect! You've selected {} for this transaction.\n\n🔐 Step 4: Biometric Authentication\n\nTo complete this booking, I'll need to enroll your payment card with biometric authentication.\n\nReady to proceed with payment enrollment? (Yes/No)",
                        selected_method
                    );
                    updated_messages.push(ClaudeMessage {
                        role: "assistant".to_string(),
                        content: response.clone(),
                    });
                    return Ok((response, updated_messages, state.clone()));
                }
                
                Ok((format!("Agent A: {}", claude_response), updated_messages, state.clone()))
            } else {
                // Check if this is a pricing inquiry (get-ticket-price)
                let is_pricing_request = tool_calls.iter()
                    .any(|(name, _)| name == "get-ticket-price");

                if is_pricing_request {
                    // Execute pricing tool and return result with booking prompt
                    let mut pricing_result = None;
                    let mut from = String::new();
                    let mut to = String::new();
                    let mut price = 0.0;
                    
                    for (tool_name, arguments) in &tool_calls {
                        if tool_name == "get-ticket-price" {
                            if let Some(f) = arguments.get("from").and_then(|v| v.as_str()) {
                                from = f.to_string();
                            }
                            if let Some(t) = arguments.get("to").and_then(|v| v.as_str()) {
                                to = t.to_string();
                            }

                            match call_server_tool_with_proof(
                                &client,
                                &config.server_url,
                                &agent_b_url,
                                payment_agent_url,
                                zkfetch_wrapper_url,
                                tool_name,
                                arguments.clone(),
                            )
                            .await
                            {
                                Ok((result, proof)) => {
                                    // Collect and submit cryptographic proof if available
                                    if let Some(crypto_proof) = proof {
                                        state.cryptographic_traces.push(crypto_proof.clone());
                                        println!("[PROOF] Collected proof for {}: {}", tool_name, state.cryptographic_traces.len());
                                        
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
                                            .unwrap_or_else(|_| "http://localhost:8001".to_string());
                                        let session_id_attest = session_id.to_string();
                                        let client_attest = reqwest::Client::new();
                                        let crypto_proof_attest = crypto_proof.clone();
                                        
                                        // Capture the proof_id for reference (proof already in cryptographic_traces)
                                        let _proof_id_capture = crypto_proof.proof_id.clone();
                                        
                                        tokio::spawn(async move {
                                            match submit_proof_to_attestation_service(
                                                &client_attest,
                                                &attestation_url,
                                                &session_id_attest,
                                                &crypto_proof_attest
                                            ).await {
                                                Ok(proof_id) => {
                                                    println!("[PROOF] Submitted proof to attestation service: {}", proof_id);
                                                }
                                                Err(e) => {
                                                    eprintln!("[PROOF] Failed to submit proof to attestation service: {}", e);
                                                }
                                            }
                                        });
                                        
                                        // Store proof_id in map for payment verification
                                        // Note: proof is already stored in state.cryptographic_traces above
                                        // Single source of truth is cryptographic_traces, not a separate proof_map
                                    }
                                    
                                    if let Ok(parsed) = serde_json::from_str::<Value>(&result) {
                                        if let Some(p) = parsed.get("price").and_then(|v| v.as_f64()) {
                                            price = p;
                                        }
                                    }
                                    pricing_result = Some(result);
                                }
                                Err(e) => {
                                    let err_msg = format!("Error fetching pricing: {}", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_msg.clone(),
                                    });
                                    return Ok((err_msg, updated_messages, state.clone()));
                                }
                            }
                        }
                    }

                    if let Some(_pricing) = pricing_result {
                        // Update state with pricing information
                        state.step = "pricing".to_string();
                        state.from = from.clone();
                        state.to = to.clone();
                        state.price = price;
                        // Proof already stored in cryptographic_traces when it was captured
                        
                        let response = format!(
                            "Agent A: Great! I found a flight from {} to {} for ${}.\n\nThis includes all taxes and fees.\n\nTo complete your booking, please provide:\n1. Your full name\n2. Your email address\n\nGive me your fullname first!",
                            from, to, price
                        );
                        
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        
                        return Ok((response, updated_messages, state.clone()));
                    }

                    updated_messages.push(ClaudeMessage {
                        role: "assistant".to_string(),
                        content: claude_response.clone(),
                    });
                    Ok((format!("Agent A: {}", claude_response), updated_messages, state.clone()))
                } else {
                    // Check if this is a booking confirmation with payment
                    let is_booking_with_payment = tool_calls.iter()
                        .any(|(name, _)| name == "enroll-card" || name == "initiate-purchase-instruction" || name == "book-flight");

                    if is_booking_with_payment {
                        // Payment method selection and enrollment
                        // let payment_method = user_query.trim().to_lowercase();
                        
                        // // User selected payment method. Ask for enrollment confirmation.
                        // if state.step == "payment_method" {
                        //     // Check if user actually responded to payment method question
                        //     if !payment_method.contains("1") && !payment_method.contains("2") 
                        //         && !payment_method.contains("visa") && !payment_method.contains("other")
                        //         && !payment_method.contains("credit") && !payment_method.contains("card") {
                        //         // User didn't answer the payment method question clearly
                        //         let response = "Agent A: I need you to select your payment method. Please reply with:\n1. Visa Credit Card\n2. Other payment method".to_string();
                        //         updated_messages.push(ClaudeMessage {
                        //             role: "assistant".to_string(),
                        //             content: response.clone(),
                        //         });
                        //         return Ok((response, updated_messages, state.clone()));
                        //     }
                            
                        //     let selected_method = if payment_method.contains("1") || payment_method.contains("visa") {
                        //         "Visa Credit Card"
                        //     } else {
                        //         "Visa Credit Card" // Default to Visa if other selected
                        //     };
                            
                        //     // Update state with payment method
                        //     state.step = "enrollment_confirmation".to_string();
                        //     state.payment_method = Some(selected_method.to_string());
                            
                        //     let response = format!(
                        //         "Agent A: Perfect! You've selected {} for this transaction.\n\n🔐 Step 4: Biometric Authentication\n\nTo complete this booking, I'll need to enroll your payment card with biometric authentication.\n\nReady to proceed with payment enrollment? (Yes/No)",
                        //         selected_method
                        //     );
                        //     updated_messages.push(ClaudeMessage {
                        //         role: "assistant".to_string(),
                        //         content: response.clone(),
                        //     });
                        //     return Ok((response, updated_messages, state.clone()));
                        // }
                        
                        // User confirmed enrollment. Now proceed with full payment (Turn 6)
                        if state.step == "enrollment_confirmation" {
                            // First check if user is responding to the enrollment confirmation prompt
                            let response_lower = user_query.trim().to_lowercase();
                            
                            if !response_lower.contains("yes") && !response_lower.contains("ok") && !response_lower.contains("confirm") && !response_lower.contains("proceed") && !response_lower.contains("y") {
                                // User didn't confirm, ask again
                                let response = "Agent A: I need your confirmation to proceed. Are you ready to proceed with payment enrollment? (Yes/No)".to_string();
                                updated_messages.push(ClaudeMessage {
                                    role: "assistant".to_string(),
                                    content: response.clone(),
                                });
                                return Ok((response, updated_messages, state.clone()));
                            }
                            
                            let from = state.from.clone();
                            let to = state.to.clone();
                            let price = state.price;
                            let passenger_name = state.passenger_name.clone().unwrap_or_default();
                            let passenger_email = state.passenger_email.clone().unwrap_or_default();

                            // Update state to payment
                            state.step = "payment".to_string();

                            match complete_booking_with_payment(
                                config,
                                session_id,
                                &from,
                                &to,
                                price,
                                &passenger_name,
                                &passenger_email,
                                state,
                            )
                            .await
                            {
                                Ok(result) => {
                                    state.step = "completed".to_string();
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: result.clone(),
                                    });
                                    Ok((result, updated_messages, state.clone()))
                                }
                                Err(e) => {
                                    let err_response = format!("Agent A: There was an issue processing your booking: {}\n\nPlease try again or contact support.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    Ok((err_response, updated_messages, state.clone()))
                                }
                            }
                        } else {
                            // Fallback: shouldn't reach here, but handle gracefully
                            let response = "Agent A: I'm ready to help with your booking. Could you please confirm your enrollment details?".to_string();
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            Ok((response, updated_messages, state.clone()))
                        }
                    } else {
                        // Non-pricing, non-booking tool flow - execute all tools
                        let mut results = Vec::new();
                        
                        for (tool_name, arguments) in &tool_calls {
                            match call_server_tool(
                                &client,
                                &config.server_url,
                                &agent_b_url,
                                payment_agent_url,
                                tool_name,
                                arguments.clone(),
                            )
                            .await
                            {
                                Ok(result) => {
                                    results.push(format!("Tool: {} | Result: {}", tool_name, result));
                                }
                                Err(e) => {
                                    results.push(format!("Tool: {} | Error: {}", tool_name, e));
                                }
                            }
                        }

                        // Extract user message from Claude response if available
                        let response = if let Ok(parsed) = serde_json::from_str::<Value>(&claude_response) {
                            if let Some(msg) = parsed.get("user_message").and_then(|m| m.as_str()) {
                                format!("Agent A: {}\n\nResults:\n{}", msg, results.join("\n"))
                            } else {
                                format!("Agent A: {}\n\nResults:\n{}", claude_response, results.join("\n"))
                            }
                        } else {
                            format!("Agent A: {}\n\nResults:\n{}", claude_response, results.join("\n"))
                        };
                        
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        
                        Ok((response, updated_messages, state.clone()))
                    }
                }
            }
        }
        Err(e) => {
            // Parse failed, log details and return raw response
            eprintln!("[PARSE ERROR] Failed to parse tool calls: {}", e);
            eprintln!("[PARSE ERROR] Claude response (first 500 chars): {}", 
                     &claude_response[..claude_response.len().min(500)]);
            
            let response = format!("Agent A: {}", claude_response);
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            Ok((response, updated_messages, state.clone()))
        }
    }
}

// }

