/// Tool options mapping module
/// Defines privacy-preserving redaction rules for cryptographic proofs

use std::collections::HashMap;
use serde_json::json;
use crate::proxy_fetch::{ZkfetchToolOptions, ToolOptionsMap};

/// Build a map of tool-specific redaction rules for privacy-preserving proofs
/// 
/// This defines which sensitive fields should be masked in cryptographic proofs
/// for each MCP tool. The redaction rules use dot-notation paths to specify fields.
/// 
/// # Tool Redaction Rules
/// 
/// - **get-ticket-price**: No redactions (pricing is public info)
/// - **book-flight**: Masks passenger_name and passenger_email
/// - **enroll-card**: Masks card_number, cvv, expiry
/// - **initiate-purchase-instruction**: Masks amount, tokenId
/// - **retrieve-payment-credentials**: Masks tokenId, instructionId, credentials
pub fn build_tool_options_map() -> ToolOptionsMap {
    let mut map = HashMap::new();

    // get-ticket-price: Pricing query - no sensitive data
    // Pricing information is public and doesn't need redaction
    map.insert(
        "get-ticket-price".to_string(),
        ZkfetchToolOptions::default(),
    );

    // book-flight: Passenger booking - redact PII
    // Reveals ONLY: booking_id, confirmation_code, status
    // Hides: passenger_name, from, to, and other details
    let mut book_flight_paths = HashMap::new();
    book_flight_paths.insert("booking_id".to_string(), "$.data.booking_id".to_string());
    
    map.insert(
        "book-flight".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive request body from proof
            // This keeps passenger PII out of the on-chain proof
            private_options: Some(json!({
                "hiddenParameters": ["passenger_name", "passenger_email"]
            })),
            // Select ONLY the fields we want to reveal - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.booking_id"}),
                json!({"jsonPath": "$.data.confirmation_code"}),
                json!({"jsonPath": "$.data.status"}),
            ]),
            response_redaction_paths: Some(book_flight_paths),
        },
    );

    // enroll-card: Payment card enrollment - redact card details
    // Reveals ONLY: tokenId
    // Hides: all card information from proof
    let mut enroll_card_paths = HashMap::new();
    enroll_card_paths.insert("tokenId".to_string(), "$.data.tokenId".to_string());
    
    map.insert(
        "enroll-card".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive card data from proof
            private_options: Some(json!({
                "hiddenParameters": ["card_number", "cvv", "expiry"]
            })),
            // Select ONLY the token ID - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.tokenId"}),
            ]),
            response_redaction_paths: Some(enroll_card_paths),
        },
    );

    // initiate-purchase-instruction: Payment initiation - redact transaction details
    // Reveals ONLY: instructionId
    // Hides: amount, tokenId, and other sensitive transaction details from proof
    let mut purchase_paths = HashMap::new();
    purchase_paths.insert("instructionId".to_string(), "$.data.instructionId".to_string());
    
    map.insert(
        "initiate-purchase-instruction".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive transaction data from proof
            private_options: Some(json!({
                "hiddenParameters": ["amount", "tokenId"]
            })),
            // Select ONLY the instruction ID - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.instructionId"}),
            ]),
            response_redaction_paths: Some(purchase_paths),
        },
    );

    // retrieve-payment-credentials: Payment credential retrieval - redact all sensitive data
    // Reveals ONLY: credentials
    // Hides: tokenId, instructionId, and other identifiers from proof
    let mut credentials_paths = HashMap::new();
    credentials_paths.insert("credentials".to_string(), "$.data.credentials".to_string());
    
    map.insert(
        "retrieve-payment-credentials".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive identifiers from proof
            private_options: Some(json!({
                "hiddenParameters": ["tokenId", "instructionId"]
            })),
            // Select ONLY the credentials - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.credentials"}),
            ]),
            response_redaction_paths: Some(credentials_paths),
        },
    );

    map
}
