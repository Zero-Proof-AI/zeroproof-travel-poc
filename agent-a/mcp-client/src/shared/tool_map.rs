/// Tool options mapping module
/// Defines privacy-preserving redaction rules for cryptographic proofs

use std::collections::HashMap;
use serde_json::json;
use crate::shared::proxy_fetch::{ZkfetchToolOptions, ToolOptionsMap};

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
        },
    );

    // enroll-card: Payment card enrollment - redact card details
    // Reveals ONLY: tokenId
    // Hides: all card information from proof
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
        },
    );

    // initiate-purchase-instruction: Payment initiation - redact transaction details
    // Reveals ONLY: instructionId
    // Hides: amount, tokenId, and other sensitive transaction details from proof
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
        },
    );

    // retrieve-payment-credentials: Payment credential retrieval - redact sensitive identifiers
    // Reveals ONLY: status, instructionId, authorization (proof of successful payment)
    // Hides: tokenId, signedPayload, and other sensitive identifiers from proof
    // NOTE: The on-chain proof will have placeholder values for hidden parameters,
    // and the contract should verify the identifier against keccak256(claimInfo with placeholders)
    map.insert(
        "retrieve-payment-credentials".to_string(),
        ZkfetchToolOptions {
            public_options: None,
            // Use private_options to hide sensitive identifiers from proof
            private_options: Some(json!({
                "hiddenParameters": ["tokenId", "signedPayload", "transactionReferenceId"]
            })),
            // Select ONLY the fields that prove payment was successful - everything else is redacted
            redactions: Some(vec![
                json!({"jsonPath": "$.data.status"}),
                json!({"jsonPath": "$.data.instructionId"}),
                json!({"jsonPath": "$.data.authorization"}),
            ]),
        },
    );

    map
}
