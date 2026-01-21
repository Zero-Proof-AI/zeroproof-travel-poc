/// Orchestration logic for Agent A - extracted from main.rs for reuse
/// This module contains all the core agent logic:
/// - Claude API calls
/// - Tool routing and execution
/// - Payment workflows
/// - Proxy-fetch integration

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::shared::{fetch_all_tools, parse_tool_calls, call_claude, call_server_tool, CryptographicProof};
use crate::prompts::extract_with_claude;
use crate::booking::{complete_booking, get_ticket_pricing};
use crate::payment::{enroll_card_if_needed, initiate_payment, retrieve_payment_credentials};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: String,
}

/// Booking state tracking across multi-turn conversations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingState {
    pub step: String, // "initial", "pricing", "passenger_name", "passenger_email", "payment_method", "enrollment_confirmation", "enrollment", "initiate-payment", "retrieve-credentials", "payment", "completed"
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

// Note: Proofs are now delivered in real-time via progress channel in submit_proof_to_database_with_progress()
// No need for batch fetching at the end of operations

/// Process a user query through the full orchestration pipeline
/// Handles multi-turn conversations including booking workflows
/// Returns (response_text, updated_messages, updated_state)
pub async fn process_user_query(
    config: &AgentConfig,
    user_query: &str,
    messages: &[ClaudeMessage],
    state: &mut BookingState,
    session_id: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<String>>,
) -> Result<(String, Vec<ClaudeMessage>, BookingState)> {
    let client = reqwest::Client::new();

    // Helper to send progress updates
    async fn send_progress(tx: &Option<tokio::sync::mpsc::Sender<String>>, msg: &str) {
        if let Some(sender) = tx {
            let _ = sender.send(msg.to_string()).await;
        }
    }

    send_progress(&progress_tx, "🔄 Processing request...").await;

    // Fetch tool definitions
    let agent_b_url = std::env::var("AGENT_B_MCP_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());
    
    let payment_agent_url = if config.payment_agent_enabled {
        config.payment_agent_url.as_deref()
    } else {
        None
    };

    let tool_definitions = fetch_all_tools(&client, &config.server_url, &agent_b_url, payment_agent_url).await?;

    // Check if we're in an extraction state - if so, skip tool-based Claude call and go straight to extraction
    if (state.step == "passenger_name" && state.passenger_name.is_none()) ||
       (state.step == "passenger_email" && state.passenger_email.is_none()) ||
       (state.step == "payment_method") {
        println!("[EXTRACTION MODE] Detected extraction state: '{}' - skipping tool-based Claude call", state.step);
        send_progress(&progress_tx, "📝 Extracting passenger information...").await;
        
        let mut updated_messages = messages.to_vec();
        
        if state.step == "passenger_name" && state.passenger_name.is_none() {
            println!("[CONDITION CHECK] ✓ Name condition matched: step='{}' | passenger_name.is_none()={}", 
                     state.step, state.passenger_name.is_none());
            let extracted_name = extract_with_claude(&client, config, "passenger_name", user_query, state, &serde_json::json!([])).await?;
            
            if !extracted_name.is_empty() {
                println!("[STATE_TRANSITION] Name extracted: '{}' → state.step now = 'passenger_email'", extracted_name);
                state.passenger_name = Some(extracted_name.clone());
                state.step = "passenger_email".to_string();
                
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
                let response = "Agent A: I couldn't understand that. Could you please provide your full name?".to_string();
                updated_messages.push(ClaudeMessage {
                    role: "assistant".to_string(),
                    content: response.clone(),
                });
                return Ok((response, updated_messages, state.clone()));
            }
        }
        
        if state.step == "passenger_email" && state.passenger_email.is_none() {
            println!("[CONDITION CHECK] ✓ Email condition matched: step='{}' | passenger_email.is_none()={}", 
                     state.step, state.passenger_email.is_none());
            let extracted_email = extract_with_claude(&client, config, "passenger_email", user_query, state, &serde_json::json!([])).await?;
            
            if !extracted_email.is_empty() {
                println!("[STATE_TRANSITION] Email extracted: '{}' → state.step now = 'payment_method'", extracted_email);
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
                let response = "Agent A: I couldn't find a valid email. Please provide your email address (e.g., user@example.com):".to_string();
                updated_messages.push(ClaudeMessage {
                    role: "assistant".to_string(),
                    content: response.clone(),
                });
                return Ok((response, updated_messages, state.clone()));
            }
        }
        
        if state.step == "payment_method" {
            println!("[CONDITION CHECK] ✓ Payment method condition matched: step='{}'", state.step);
            let payment_method = extract_with_claude(&client, config, "payment_method", user_query, state, &serde_json::json!([])).await?;
            println!("[DEBUG] Extracted payment_method: '{}' (empty: {})", payment_method, payment_method.is_empty());
            
            // Check if extraction returned empty
            if payment_method.is_empty() {
                let response = "Agent A: I need you to select your payment method. Please reply with:\n1. Visa Credit Card\n2. Other payment method".to_string();
                updated_messages.push(ClaudeMessage {
                    role: "assistant".to_string(),
                    content: response.clone(),
                });
                return Ok((response, updated_messages, state.clone()));
            }
            
            // Payment method already extracted and converted by extract_with_claude
            state.step = "enrollment_confirmation".to_string();
            state.payment_method = Some(payment_method.clone());
            
            let response = format!(
                "Agent A: Perfect! You've selected {} for this transaction.\n\n🔐 Step 4: Biometric Authentication\n\nTo complete this booking, I'll need to enroll your payment card with biometric authentication.\n\nReady to proceed with payment enrollment? (Yes/No)",
                payment_method
            );
            updated_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            return Ok((response, updated_messages, state.clone()));
        }
    }

    // Call Claude with full message history
    let claude_response = call_claude(&client, config, user_query, messages, state, &tool_definitions, None).await?;
    send_progress(&progress_tx, "✅ Claude processed request").await;
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
                
                Ok((format!("Agent A: {}", claude_response), updated_messages, state.clone()))
            } else {
                // Check if this is a pricing inquiry (get-ticket-price)
                let is_pricing_request = tool_calls.iter()
                    .any(|(name, _)| name == "get-ticket-price");

                if is_pricing_request {
                    // Extract pricing info from Claude for context
                    if let Some((_, args)) = tool_calls.first() {
                        if let (Some(from), Some(to)) = (
                            args.get("from").and_then(|v| v.as_str()),
                            args.get("to").and_then(|v| v.as_str()),
                        ) {
                            state.from = from.to_string();
                            state.to = to.to_string();
                            state.step = "passenger_name".to_string();
                            
                            send_progress(&progress_tx, &format!("🔍 Fetching pricing for {} → {}", from, to)).await;
                            
                            // Call get_ticket_pricing to fetch pricing and collect proof
                            match get_ticket_pricing(&config, &session_id, from, to, state, progress_tx.clone()).await {
                                Ok(pricing_result) => {
                                    // send_progress(&progress_tx, "💰 Pricing received").await;
                                    println!("[PRICING] Fetched: {}", pricing_result);
                                    
                                    // Parse pricing result to extract price
                                    if let Ok(price_json) = serde_json::from_str::<Value>(&pricing_result) {
                                        if let Some(price) = price_json.get("price").and_then(|p| p.as_f64()) {
                                            state.price = price;
                                            println!("[PRICING] Stored price: ${}", price);
                                        }
                                    }
                                    
                                    // Add assistant message showing pricing result
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: format!("Great! I found a flight from {} to {} for ${}.", from, to, state.price),
                                    });
                                    
                                    // Ask for passenger name
                                    let response = format!("Agent A: Perfect! I found a flight from {} to {} for ${:.2}.\n\nNow, please provide your full name:", from, to, state.price);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: response.clone(),
                                    });
                                    return Ok((response, updated_messages, state.clone()));
                                }
                                Err(e) => {
                                    eprintln!("[PRICING] Error: {}", e);
                                    let response = format!("Agent A: Sorry, I couldn't fetch the pricing information: {}", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: response.clone(),
                                    });
                                    return Ok((response, updated_messages, state.clone()));
                                }
                            }
                        } else {
                            let response = "Agent A: I need valid departure and destination cities to fetch pricing.".to_string();
                            updated_messages.push(ClaudeMessage {
                                role: "assistant".to_string(),
                                content: response.clone(),
                            });
                            return Ok((response, updated_messages, state.clone()));
                        }
                    } else {
                        let response = "Agent A: I couldn't extract the flight details. Please provide departure and destination cities.".to_string();
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        return Ok((response, updated_messages, state.clone()));
                    }
                } else {
                    // Check if this is a booking confirmation with payment
                    let is_booking_with_payment = tool_calls.iter()
                        .any(|(name, _)| name == "enroll-card" || name == "initiate-purchase-instruction" || name == "book-flight");

                    if is_booking_with_payment {
                        // Payment method selection and enrollment
                        let payment_method = user_query.trim().to_lowercase();
                        
                        // // User selected payment method. Ask for enrollment confirmation.
                        if state.step == "payment_method" {
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
                            
                            let passenger_name = state.passenger_name.clone().unwrap_or_default();

                            // Update state to enrollment
                            state.step = "enrollment".to_string();
                            send_progress(&progress_tx, "🔐 Enrolling card for payment... (Please wait, it takes time to generate ZKP)").await;

                            // Enroll card
                            let payment_agent_url = if config.payment_agent_enabled {
                                config.payment_agent_url.as_deref()
                            } else {
                                None
                            };

                            let enrollment_result = enroll_card_if_needed(
                                &config,
                                &session_id,
                                payment_agent_url,
                                state,
                                progress_tx.clone(),
                            ).await;

                            match enrollment_result {
                                Ok((token_id, enrollment_complete)) => {
                                    println!("[ORCHESTRATION] Card enrollment complete: {} (token: {})", enrollment_complete, token_id);
                                    send_progress(&progress_tx, &format!("✅ Card enrolled: {}", token_id)).await;
                                    
                                    // Store token and update state for next step
                                    state.enrollment_token_id = Some(token_id);
                                    state.step = "initiate-payment".to_string();
                                    
                                    // Continue to payment initiation step automatically (don't return)
                                }
                                Err(e) => {
                                    eprintln!("[ORCHESTRATION] Card enrollment failed: {}", e);
                                    send_progress(&progress_tx, &format!("❌ Card enrollment failed: {}", e)).await;
                                    let err_response = format!("Agent A: Card enrollment failed: {}.\n\nWould you like to retry enrollment or restart the booking process?\n\nReply with 'retry' to try again, or 'restart' to begin over.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    return Ok((err_response, updated_messages, state.clone()));
                                }
                            }
                        }
                        
                        if state.step == "initiate-payment" {
                            // Payment initiation step - separate from booking
                            let from = state.from.clone();
                            let to = state.to.clone();
                            let price = state.price;
                            let passenger_name = state.passenger_name.clone().unwrap_or_default();
                            let passenger_email = state.passenger_email.clone().unwrap_or_default();
                            let enrollment_token_id = state.enrollment_token_id.clone().unwrap_or_default();

                            send_progress(&progress_tx, &format!("💳 Initiating payment for ${:.2}...(Please wait, it takes time to generate ZKP)", price)).await;
                            
                            let payment_agent_url = if config.payment_agent_enabled {
                                config.payment_agent_url.as_deref()
                            } else {
                                None
                            };

                            match initiate_payment(
                                config,
                                &session_id,
                                &enrollment_token_id,
                                price,
                                payment_agent_url,
                            ).await {
                                Ok(instruction_id) => {
                                    println!("[ORCHESTRATION] Payment initiated with instruction_id: {}", instruction_id);
                                    send_progress(&progress_tx, &format!("✅ Payment initiated: ${:.2}", price)).await;
                                    
                                    // Store instruction_id and update state for next step
                                    state.instruction_id = Some(instruction_id);
                                    state.step = "retrieve-credentials".to_string();
                                    
                                    // Continue to credential retrieval step automatically (don't return)
                                }
                                Err(e) => {
                                    eprintln!("[ORCHESTRATION] Payment initiation failed: {}", e);
                                    send_progress(&progress_tx, &format!("❌ Payment initiation failed: {}", e)).await;
                                    let err_response = format!("Agent A: Payment initiation failed: {}.\n\nWould you like to retry or restart the booking process?\n\nReply with 'retry' to try again, or 'restart' to begin over.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    return Ok((err_response, updated_messages, state.clone()));
                                }
                            }
                        }
                        
                        if state.step == "retrieve-credentials" {
                            // Retrieve payment credentials step - separate from booking
                            let enrollment_token_id = state.enrollment_token_id.clone().unwrap_or_default();
                            let instruction_id = state.instruction_id.clone().unwrap_or_default();

                            send_progress(&progress_tx, "🔑 Retrieving payment credentials...").await;

                            let payment_agent_url = if config.payment_agent_enabled {
                                config.payment_agent_url.as_deref()
                            } else {
                                None
                            };

                            match retrieve_payment_credentials(
                                config,
                                &session_id,
                                &enrollment_token_id,
                                &instruction_id,
                                payment_agent_url,
                            ).await {
                                Ok(_) => {
                                    println!("[ORCHESTRATION] Payment credentials retrieved");
                                    send_progress(&progress_tx, "✅ Payment credentials retrieved").await;
                                    
                                    // Update state for next step
                                    state.step = "payment".to_string();
                                    
                                    // Continue to payment step automatically (don't return)
                                }
                                Err(e) => {
                                    eprintln!("[ORCHESTRATION] Credential retrieval failed: {}", e);
                                    send_progress(&progress_tx, &format!("❌ Failed to retrieve credentials: {}", e)).await;
                                    let err_response = format!("Agent A: Credential retrieval failed: {}.

Would you like to retry or restart the booking process?

Reply with 'retry' to try again, or 'restart' to begin over.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    return Ok((err_response, updated_messages, state.clone()));
                                }
                            }
                        }
                        
                        if state.step == "payment" {
                            // Complete booking step - flight booking only
                            let from = state.from.clone();
                            let to = state.to.clone();
                            let price = state.price;
                            let passenger_name = state.passenger_name.clone().unwrap_or_default();
                            let passenger_email = state.passenger_email.clone().unwrap_or_default();
                            let enrollment_token_id = state.enrollment_token_id.clone().unwrap_or_default();
                            let instruction_id = state.instruction_id.clone().unwrap_or_default();

                            send_progress(&progress_tx, &format!("✈️  Completing booking for {} booking", passenger_name)).await;

                            // Complete booking with payment (credentials retrieval + flight booking)
                            match complete_booking(
                                config,
                                &session_id,
                                &from,
                                &to,
                                price,
                                &passenger_name,
                                &passenger_email,
                                &enrollment_token_id,
                                &instruction_id,
                                state,
                                progress_tx.clone(),
                            )
                            .await
                            {
                                Ok(result) => {
                                    send_progress(&progress_tx, "✅ Booking completed successfully").await;
                                    
                                    state.step = "completed".to_string();
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: result.clone(),
                                    });
                                    return Ok((result, updated_messages, state.clone()));
                                }
                                Err(e) => {
                                    let err_response = format!("Agent A: There was an issue processing your booking: {}\n\nPlease try again or contact support.", e);
                                    updated_messages.push(ClaudeMessage {
                                        role: "assistant".to_string(),
                                        content: err_response.clone(),
                                    });
                                    return Ok((err_response, updated_messages, state.clone()));
                                }
                            }
                        }
                        
                        // If we reach here, none of the payment steps matched
                        // This shouldn't happen in normal flow
                        let response = "Agent A: I'm ready to help with your booking.".to_string();
                        updated_messages.push(ClaudeMessage {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        Ok((response, updated_messages, state.clone()))
                    } else {
                        // Non-pricing, non-booking tool flow - execute all tools
                        let mut results = Vec::new();
                        
                        for (tool_name, arguments) in &tool_calls {
                            send_progress(&progress_tx, &format!("🔧 Calling tool: {}", tool_name)).await;
                            
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
                                    send_progress(&progress_tx, &format!("✅ {} completed", tool_name)).await;
                                    results.push(format!("Tool: {} | Result: {}", tool_name, result));
                                }
                                Err(e) => {
                                    send_progress(&progress_tx, &format!("❌ {} failed: {}", tool_name, e)).await;
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

