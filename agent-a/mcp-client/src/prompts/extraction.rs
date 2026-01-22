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
         User may respond with:\n\
         - '1' or 'Visa' or 'Visa Credit Card' → respond with 'Visa Credit Card'\n\
         - '2' or 'Other' → respond with 'Other'\n\n\
         Respond with ONLY the payment method name, nothing else. \
         If no payment method is provided, respond with: NONE",
        user_input
    )
}

/// Get prompt for extracting departure city from user input
pub fn get_departure_city_extraction_prompt(user_input: &str) -> String {
    format!(
        "Extract the departure city from this user input: \"{}\"\n\n\
         Common city codes: NYC (New York), LAX (Los Angeles), ORD (Chicago), SFO (San Francisco), \
         LON (London), PAR (Paris), LHR (London), CDG (Paris), NRT (Tokyo), SYD (Sydney), DXB (Dubai), SIN (Singapore), BOS (Boston)\n\n\
         Respond with ONLY the city code (e.g., 'NYC', 'LAX', 'BOS'), nothing else. \
         If no departure city is provided, respond with: NONE",
        user_input
    )
}

/// Get prompt for extracting departure city from user input with context
pub fn get_departure_city_extraction_prompt_with_context(user_input: &str, known_destination: Option<&str>) -> String {
    let context = if let Some(dest) = known_destination {
        format!("Context: The destination city is already set to {}. ", dest)
    } else {
        String::new()
    };
    
    format!(
        "{}Extract the departure city from this user input: \"{}\"\n\n\
         Common city codes: NYC (New York), LAX (Los Angeles), ORD (Chicago), SFO (San Francisco), \
         LON (London), PAR (Paris), LHR (London), CDG (Paris), NRT (Tokyo), SYD (Sydney), DXB (Dubai), SIN (Singapore), BOS (Boston)\n\n\
         Respond with ONLY the city code (e.g., 'NYC', 'LAX', 'BOS'), nothing else. \
         If no departure city is provided, respond with: NONE",
        context, user_input
    )
}

/// Get prompt for extracting destination city from user input
pub fn get_destination_city_extraction_prompt(user_input: &str) -> String {
    format!(
        "Extract the destination city from this user input: \"{}\"\n\n\
         Common city codes: NYC (New York), LAX (Los Angeles), ORD (Chicago), SFO (San Francisco), \
         LON (London), PAR (Paris), LHR (London), CDG (Paris), NRT (Tokyo), SYD (Sydney), DXB (Dubai), SIN (Singapore), BOS (Boston)\n\n\
         Respond with ONLY the city code (e.g., 'NYC', 'LAX', 'BOS'), nothing else. \
         If no destination city is provided, respond with: NONE",
        user_input
    )
}

/// Get prompt for extracting destination city from user input with context
pub fn get_destination_city_extraction_prompt_with_context(user_input: &str, known_departure: Option<&str>) -> String {
    let context = if let Some(dep) = known_departure {
        format!("Context: The departure city is already set to {}. ", dep)
    } else {
        String::new()
    };
    
    format!(
        "{}Extract the destination city from this user input: \"{}\"\n\n\
         Common city codes: NYC (New York), LAX (Los Angeles), ORD (Chicago), SFO (San Francisco), \
         LON (London), PAR (Paris), LHR (London), CDG (Paris), NRT (Tokyo), SYD (Sydney), DXB (Dubai), SIN (Singapore), BOS (Boston)\n\n\
         Respond with ONLY the city code (e.g., 'NYC', 'LAX', 'BOS'), nothing else. \
         If no destination city is provided, respond with: NONE",
        context, user_input
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
        "departure_city" => get_departure_city_extraction_prompt(user_input),
        "destination_city" => get_destination_city_extraction_prompt(user_input),
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
    println!("[DEBUG] Claude response for {}: '{}' (length: {})", field, claude_response, claude_response.len());

    let trimmed = claude_response.trim();
    println!("[DEBUG] Trimmed response: '{}' (length: {})", trimmed, trimmed.len());
    
    if trimmed.to_uppercase() == "NONE" || trimmed.is_empty() {
        println!("[DEBUG] Returning empty string for {}", field);
        Ok(String::new())
    } else {
        // Additional validation for email
        if field == "passenger_email" {
            let email_trimmed = trimmed
                .to_lowercase()
                .trim()
                .trim_matches(|c| c == '"' || c == '\'' || c == '*' || c == '_')
                .to_string();
            
            // Basic email validation
            if email_trimmed.contains('@') && email_trimmed.contains('.') {
                println!("[DEBUG] Valid email extracted for {}: '{}'", field, email_trimmed);
                return Ok(email_trimmed);
            } else {
                println!("[DEBUG] Invalid email format: '{}' - returning empty", email_trimmed);
                return Ok(String::new());
            }
        }
        
        println!("[DEBUG] Returning extracted {}: '{}'", field, trimmed);
        
        // Additional validation for payment_method
        if field == "payment_method" {
            let payment_lower = trimmed.to_lowercase();
            // Convert numbered responses to payment method names
            if payment_lower.contains('1') || payment_lower.contains("visa") || payment_lower.contains("credit") {
                return Ok("Visa Credit Card".to_string());
            } else if payment_lower.contains('2') || payment_lower.contains("other") {
                return Ok("Other".to_string());
            }
            // If we got a clear response, return it
            if trimmed != "NONE" && !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }

        // Validation for city codes - ensure uppercase 3-letter codes
        if field == "departure_city" || field == "destination_city" {
            let city_code = trimmed.to_uppercase();
            if city_code.len() == 3 && city_code.chars().all(|c| c.is_alphabetic()) {
                println!("[DEBUG] Valid city code extracted for {}: '{}'", field, city_code);
                return Ok(city_code);
            } else {
                println!("[DEBUG] Invalid city code format: '{}' - returning empty", city_code);
                return Ok(String::new());
            }
        }
        
        Ok(trimmed.to_string())
    }
}
