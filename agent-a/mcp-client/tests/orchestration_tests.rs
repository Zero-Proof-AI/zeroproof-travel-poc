/// Unit tests for orchestration module
/// Tests the build_tool_options_map function and related redaction logic

mod common;

use mcp_client::orchestration::build_tool_options_map;
use serde_json::json;

#[test]
fn test_build_tool_options_map_contains_all_tools() {
    let tool_map = build_tool_options_map();
    
    // Verify all expected tools are in the map
    assert!(tool_map.contains_key("get-ticket-price"));
    assert!(tool_map.contains_key("book-flight"));
    assert!(tool_map.contains_key("enroll-card"));
    assert!(tool_map.contains_key("initiate-purchase-instruction"));
    assert!(tool_map.contains_key("retrieve-payment-credentials"));
}

#[test]
fn test_build_tool_options_map_pricing_no_redactions() {
    let tool_map = build_tool_options_map();
    
    // get-ticket-price should have no redactions
    let pricing_opts = tool_map.get("get-ticket-price").unwrap();
    assert!(pricing_opts.redactions.is_none() || pricing_opts.redactions.as_ref().unwrap().is_empty());
}

#[test]
fn test_build_tool_options_map_book_flight_redactions() {
    let tool_map = build_tool_options_map();
    
    // book-flight should reveal only booking confirmation details
    let booking_opts = tool_map.get("book-flight").unwrap();
    assert!(booking_opts.redactions.is_some());
    
    let redactions = booking_opts.redactions.as_ref().unwrap();
    assert!(!redactions.is_empty());
    
    // Verify response redaction paths (fields to reveal)
    assert!(booking_opts.response_redaction_paths.is_some());
    let paths = booking_opts.response_redaction_paths.as_ref().unwrap();
    assert!(paths.contains_key("booking_id"));
}

#[test]
fn test_build_tool_options_map_enroll_card_redactions() {
    let tool_map = build_tool_options_map();
    
    // enroll-card should redact payment card details
    let enroll_opts = tool_map.get("enroll-card").unwrap();
    assert!(enroll_opts.redactions.is_some());
    
    let redactions = enroll_opts.redactions.as_ref().unwrap();
    assert!(redactions.len() >= 1);
    
    // Verify response redaction paths
    assert!(enroll_opts.response_redaction_paths.is_some());
    let paths = enroll_opts.response_redaction_paths.as_ref().unwrap();
    assert!(paths.contains_key("tokenId"));
}

#[test]
fn test_build_tool_options_map_initiate_purchase_redactions() {
    let tool_map = build_tool_options_map();
    
    // initiate-purchase-instruction should redact transaction details
    let purchase_opts = tool_map.get("initiate-purchase-instruction").unwrap();
    assert!(purchase_opts.redactions.is_some());
    
    let redactions = purchase_opts.redactions.as_ref().unwrap();
    assert!(redactions.len() >= 1);
    
    // Verify response redaction paths
    assert!(purchase_opts.response_redaction_paths.is_some());
    let paths = purchase_opts.response_redaction_paths.as_ref().unwrap();
    assert!(paths.contains_key("instructionId"));
}

#[test]
fn test_build_tool_options_map_retrieve_credentials_redactions() {
    let tool_map = build_tool_options_map();
    
    // retrieve-payment-credentials should reveal only credentials
    let retrieve_opts = tool_map.get("retrieve-payment-credentials").unwrap();
    assert!(retrieve_opts.redactions.is_some());
    
    let redactions = retrieve_opts.redactions.as_ref().unwrap();
    assert!(!redactions.is_empty());
    
    // Verify response redaction paths (fields to reveal)
    assert!(retrieve_opts.response_redaction_paths.is_some());
    let paths = retrieve_opts.response_redaction_paths.as_ref().unwrap();
    assert!(paths.contains_key("credentials"));
    
    // Verify private options hide sensitive request data
    assert!(retrieve_opts.private_options.is_some());
}

#[test]
fn test_zkfetch_payload_includes_redactions() {
    // Verify that the zkfetch payload structure includes redactions for tools
    let tool_options_map = build_tool_options_map();
    
    // book-flight should have redactions
    let book_flight_opts = tool_options_map.get("book-flight");
    assert!(book_flight_opts.is_some(), "book-flight should be in tool options map");
    
    let book_flight = book_flight_opts.unwrap();
    assert!(book_flight.redactions.is_some(), "book-flight should have redactions");
    
    let redactions = book_flight.redactions.as_ref().unwrap();
    assert!(!redactions.is_empty(), "book-flight redactions should not be empty");
    
    // Verify redaction structure has required fields
    for redaction in redactions {
        assert!(redaction.get("jsonPath").is_some(), "Each redaction must have 'jsonPath' field");
    }
    
    // Verify that redactions can be serialized to JSON for zkfetch payload
    let payload = json!({
        "url": "http://agent-b/tools/book-flight",
        "publicOptions": {
            "method": "POST",
            "headers": {"Content-Type": "application/json"},
            "body": "{}"
        },
        "redactions": redactions
    });
    
    // Verify the payload structure is correct
    assert!(payload.get("url").is_some());
    assert!(payload.get("publicOptions").is_some());
    assert!(payload.get("redactions").is_some());
    
    // Verify redactions are properly nested in payload
    let payload_redactions = payload.get("redactions").unwrap();
    assert_eq!(payload_redactions.as_array().unwrap().len(), redactions.len());
}
