use alloc::string::String;
use serde::{Deserialize, Serialize};

#[cfg(feature = "http")]
use serde_json::{json, Value};

#[cfg(feature = "http")]
use alloc::string::ToString;



#[derive(Serialize, Deserialize)]
pub struct Request {
    pub from: String,
    pub to: String,
    pub passenger_name: String,
    pub passenger_email: String,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub booking_id: String,
    pub status: String,
    pub confirmation_code: String,
}

#[cfg(feature = "http")]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BookingProof {
    pub proof: Option<Value>,
}

/// Booking logic that runs both on server and inside SP1
/// NOTE: The async version (handle_async) calls https://httpbin.org/json
/// The sync version (handle) returns a deterministic result for SP1
pub fn handle(req: Request) -> Response {
    // Deterministic booking logic for SP1 (no external HTTP calls possible)
    let booking_data = alloc::format!(
        "{}-{}-{}-{}",
        req.from, req.to, req.passenger_name, req.passenger_email
    );
    
    let booking_id = alloc::format!("BK{:08X}", booking_data.len() * 12345);
    let confirmation_code = alloc::format!("CONF{:06X}", booking_data.len() * 67890);

    Response {
        booking_id,
        status: String::from("confirmed"),
        confirmation_code,
    }
}

/// Async version for server: calls https://httpbin.org/json through zkfetch proxy
/// Generates ZK proof with selective disclosure:
/// - Extracts: slideshow title dynamically from response
/// - Redacts: sensitive data (slides, author, date) to hide from verifiers
/// Verifiers can only see the extracted title, proving it exists in the response
/// Returns: (Response, BookingProof) - response data and optional proof object
#[cfg(all(feature = "http", not(target_os = "none")))]
pub async fn handle_async(req: Request, zkfetch_url: String, session_id: &str) -> (Response, Option<Value>) {
    // Import proxy_fetch from the shared module (via FFI/linking)
    // The shared module provides ProxyFetch with zkfetch integration
    use shared::proxy_fetch;
    use std::collections::HashMap;

    let mut status = String::from("confirmed");
    let mut confirmation_code = String::from("NO_CONFIRMATION");
    let mut proof_obj: Option<Value> = None;

    // Build tool-specific ZK options for book-flight
    let mut tool_options = HashMap::new();
    
    let mut book_flight_paths = HashMap::new();
    book_flight_paths.insert("title".to_string(), "$.slideshow.slides[1].title".to_string());

    let booking_zk_options = proxy_fetch::ZkfetchToolOptions {
        public_options: None,
        private_options: Some(json!({
            "hiddenParameters": ["passenger_name", "passenger_email"],
            "responseMatches": [
                {
                    "type": "contains",
                    "value": "\"title\": \"Overview\""
                }
            ]
        })),
        redactions: Some(vec![
            json!({"jsonPath": "$.slideshow.slides[1].title"}),
        ]),
        response_redaction_paths: Some(book_flight_paths),
    };

    tool_options.insert("book-flight".to_string(), booking_zk_options);

    let proxy_config = proxy_fetch::ProxyConfig {
        url: zkfetch_url,
        proxy_type: "zkfetch".to_string(),
        username: None,
        password: None,
        tool_options_map: Some(tool_options),
        default_zk_options: None,
        debug: true,
        attestation_config: Some(proxy_fetch::AttestationConfig {
            service_url: std::env::var("ATTESTER_URL")
                .unwrap_or_else(|_| "https://dev.attester.zeroproofai.com".to_string()),
            enabled: true,  // Enable attestation for airline confirmation proofs
            workflow_stage: Some("airline_confirmation".to_string()),
            session_id: Some(session_id.to_string()),
            submitted_by: "agent-b".to_string(),
        }),
    };

    match proxy_fetch::ProxyFetch::new(proxy_config) {
        Ok(proxy_fetch) => {
            // Use GET method since we're fetching from httpbin.org/json
            match proxy_fetch.get("https://httpbin.org/json").await {
                Ok(json_response) => {
                    eprintln!("[BOOKING] ProxyFetch response: {}", serde_json::to_string_pretty(&json_response).unwrap_or_default());
                    
                    // Extract booking confirmation from response
                    if let Some(proof) = json_response.get("proof") {
                        proof_obj = Some(proof.clone());
                        if let Some(extracted_values) = proof.get("extractedParameterValues") {
                            if let Some(Value::String(booking_conf)) = extracted_values.get("title") {
                                confirmation_code = booking_conf.clone();
                                eprintln!("[BOOKING] Extracted booking confirmation: {}", booking_conf);
                            } else if let Some(Value::String(data)) = extracted_values.get("data") {
                                // If title extraction failed, try to parse data as JSON and extract title from it
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                                    if let Some(title) = parsed.get("slideshow").and_then(|s| s.get("slides")).and_then(|slides| slides.get(1)).and_then(|slide| slide.get("title")) {
                                        if let Some(title_str) = title.as_str() {
                                            confirmation_code = title_str.to_string();
                                            eprintln!("[BOOKING] Extracted booking confirmation from data: {}", title_str);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Check for proof generation success
                    if !json_response.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        status = String::from("failed");
                        eprintln!("[BOOKING] Proof generation failed");
                    } else {
                        eprintln!("[BOOKING] Proof generated successfully via ProxyFetch");
                    }
                }
                Err(e) => {
                    status = String::from("failed");
                    confirmation_code = String::from("ERROR_PROXY_REQUEST");
                    eprintln!("[BOOKING] ProxyFetch request failed: {}", e);
                }
            }
        }
        Err(e) => {
            status = String::from("failed");
            confirmation_code = String::from("FAILED_TO_CREATE_PROXY");
            eprintln!("[BOOKING] Failed to create ProxyFetch client: {}", e);
        }
    }

    // Generate booking ID deterministically
    let booking_data = alloc::format!(
        "{}-{}-{}-{}",
        req.from, req.to, req.passenger_name, req.passenger_email
    );
    let booking_id = alloc::format!("BK{:08X}", booking_data.len() * 12345);

    eprintln!("[BOOKING] result: booking_id={}, confirmation_code={}, status={}", booking_id, confirmation_code, status);

    (Response {
        booking_id,
        status,
        confirmation_code,
    }, proof_obj)
}
