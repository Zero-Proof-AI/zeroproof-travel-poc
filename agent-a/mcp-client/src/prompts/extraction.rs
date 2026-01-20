/// Extraction prompts for information gathering from user input

use anyhow::Result;
use serde_json::Value;
use crate::shared::call_claude;
use crate::orchestration::{AgentConfig, BookingState};

pub const EXTRACTION_SYSTEM_PROMPT: &str = 
    "You are a helpful assistant that extracts specific information from user input. \
     Always respond with only the requested information or 'NONE' if not found.";

/// Get prompt for extracting passenger name from user input
pub fn get_passenger_name_extraction_prompt(user_input: &str) -> String {
    format!(
        "Extract the passenger's full name from this user input: \"{}\"\n\n\
         Respond with ONLY the name, nothing else. If no name is provided, respond with: NONE",
        user_input
    )
}

/// Get prompt for extracting email address from user input
pub fn get_passenger_email_extraction_prompt(user_input: &str) -> String {
    format!(
        "Extract the email address from this user input: \"{}\"\n\n\
         Respond with ONLY the email address, nothing else. If no email is provided, respond with: NONE",
        user_input
    )
}

/// Get prompt for extracting payment method from user input
pub fn get_payment_method_extraction_prompt(user_input: &str) -> String {
    format!(
        "Extract the payment method from this user input: \"{}\"\n\n\
         Respond with ONLY the payment method (e.g., 'Visa Credit Card', 'Other'), nothing else. \
         If no payment method is provided, respond with: NONE",
        user_input
    )
}

/// Extract information from user input using Claude's understanding
pub async fn extract_with_claude(
    client: &reqwest::Client,
    config: &AgentConfig,
    field: &str,
    user_input: &str,
    _state: &BookingState,
    _tool_definitions: &Value,
) -> Result<String> {
    let extraction_prompt = match field {
        "passenger_name" => get_passenger_name_extraction_prompt(user_input),
        "passenger_email" => get_passenger_email_extraction_prompt(user_input),
        "payment_method" => get_payment_method_extraction_prompt(user_input),
        _ => return Ok(String::new()),
    };

    println!("[DEBUG] Calling Claude for {} extraction with prompt: {}", field, extraction_prompt);
    let claude_response = call_claude(
        client,
        config,
        &extraction_prompt,
        &[], // Empty messages for extraction
        _state,
        _tool_definitions,
        Some(EXTRACTION_SYSTEM_PROMPT),
    ).await.unwrap_or_default();
    println!("[DEBUG] Claude response for {}: '{}'", field, claude_response);

    let trimmed = claude_response.trim();
    println!("[DEBUG] Trimmed response: '{}'", trimmed);
    if trimmed.to_uppercase() == "NONE" || trimmed.is_empty() {
        println!("[DEBUG] Returning empty string for {}", field);
        Ok(String::new())
    } else {
        println!("[DEBUG] Returning extracted {}: '{}'", field, trimmed);
        Ok(trimmed.to_string())
    }
}
