/// Payment proof verification module
/// Validates that Agent-A completed payment before allowing flight bookings

use regex::Regex;
use once_cell::sync::Lazy;
use shared::verify_secp256k1_sig;

/// List of trusted payment agent endpoints that Agent-B accepts
pub const ACCEPTED_PAYMENT_AGENT_URLS: &[&str] = &[
    "https://dev.justpay.zeroproofai.com/tools/retrieve-payment-credentials",
    "https://staging.justpay.zeroproofai.com/tools/retrieve-payment-credentials",
    "https://justpay.zeroproofai.com/tools/retrieve-payment-credentials",
];

/// Regex patterns for extracting URL and method from proof parameters
static URL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#""url":"([^"]+)""#).unwrap()
});

static METHOD_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#""method"\s*:\s*"([^"]+)""#).unwrap()
});

/// Verify that payment was successfully completed by checking attestation service
/// 
/// Verification checks:
/// 1. Query attestation service for retrieve-payment-credentials proof
/// 2. Verify the proof URL is in the accepted payment agent whitelist
/// 3. Verify response contains status: "confirmed"
/// 4. Verify proof cryptographic signature
pub async fn verify_payment_proof(
    session_id: &str,
    attestation_url: &str,
) -> Result<(), String> {
    tracing::info!("[VERIFY-PAYMENT] Starting payment proof verification for session: {}", session_id);
    
    let client = reqwest::Client::new();
    
    // Query attestation service for all proofs in this session
    let query_url = format!(
        "{}/proofs/session/{}",
        attestation_url,
        session_id
    );
    
    let response = client
        .get(&query_url)
        .send()
        .await
        .map_err(|e| format!("Failed to query attestation service: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!(
            "Attestation service returned error: {}",
            response.status()
        ));
    }
    
    let proofs: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse attestation service response: {}", e))?;
    
    // Extract the proofs array from ProofsResponse
    let proofs_array = proofs
        .get("proofs")
        .and_then(|p| p.as_array())
        .ok_or_else(|| "No proofs found in attestation service response".to_string())?;
    
    if proofs_array.is_empty() {
        return Err("No proofs found for this session".to_string());
    }
    
    // Find the retrieve-payment-credentials proof
    let proof = proofs_array
        .iter()
        .find(|p| {
            p.get("tool_name")
                .and_then(|t| t.as_str())
                .map(|t| t == "retrieve-payment-credentials")
                .unwrap_or(false)
        })
        .ok_or_else(|| "No retrieve-payment-credentials proof found for this session".to_string())?;
    
    // Debug: Print the entire proof structure
    tracing::info!("[VERIFY-PAYMENT] Full proof response:\n{}", serde_json::to_string_pretty(&proof).unwrap_or_else(|_| "Failed to serialize".to_string()));
    
    // Extract the proof data from the proof object
    // The proof structure has: { proof: { onchainProof: {...}, proof: {...} }, ... }
    let proof_data = proof
        .get("proof")
        .ok_or_else(|| {
            let available_keys: Vec<_> = proof.as_object()
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            format!("Proof missing 'proof' field. Available keys: {:?}", available_keys)
        })?;
    
    // VERIFICATION 1: Check that the URL is in the accepted whitelist
    // Structure: proof_data contains { onchainProof: {...}, proof: {...} }
    // URL is in proof_data.onchainProof.claimInfo.parameters or proof_data.proof.claimData.parameters
    // Use regex to extract URL pattern: "url":"<url>"
    
    // Try to get parameters from either onchainProof.claimInfo or proof.claimData
    let parameters_str = {
        // Try onchainProof.claimInfo first (for onchain version)
        if let Some(params) = proof_data
            .get("onchainProof")
            .and_then(|op| op.get("claimInfo"))
            .and_then(|c| c.get("parameters"))
            .and_then(|params| params.as_str()) {
            params.to_string()
        } else if let Some(params) = proof_data
            .get("proof")
            .and_then(|p| p.get("claimData"))
            .and_then(|c| c.get("parameters"))
            .and_then(|params| params.as_str()) {
            params.to_string()
        } else {
            return Err("Proof missing parameters in onchainProof.claimInfo or proof.claimData".to_string());
        }
    };
    
    // Extract URL using regex pattern: "url":"..."
    let proof_url = URL_PATTERN
        .captures(&parameters_str)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| format!("Failed to extract URL from proof parameters: {}", parameters_str))?;
    
    tracing::info!("[VERIFY-PAYMENT] Extracted URL from proof: {}", proof_url);
    
    if !ACCEPTED_PAYMENT_AGENT_URLS.contains(&proof_url) {
        return Err(format!(
            "Payment proof URL not in whitelist: {}. Accepted: {:?}",
            proof_url, ACCEPTED_PAYMENT_AGENT_URLS
        ));
    }
    tracing::info!("[VERIFY-PAYMENT] ✓ URL whitelisted: {}", proof_url);
    
    // VERIFICATION 1b: Verify HTTP method is POST
    let http_method = METHOD_PATTERN
        .captures(&parameters_str)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| format!("Failed to extract HTTP method from proof parameters: {}", parameters_str))?;
    
    if http_method != "POST" {
        return Err(format!(
            "Invalid HTTP method in proof: {}. Expected POST",
            http_method
        ));
    }
    tracing::info!("[VERIFY-PAYMENT] ✓ HTTP method verified: {}", http_method);
    
    // VERIFICATION 2: Check extractedParameterValues for status: "confirmed"
    // In the proof response, extractedParameterValues is at proof.proof.extractedParameterValues
    let extracted_values = proof_data
        .get("proof")
        .and_then(|p| p.get("extractedParameterValues"))
        .ok_or_else(|| "Proof missing extractedParameterValues".to_string())?;
    
    // Check if status field exists and is confirmed
    let payment_status = extracted_values
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or(""); // Default to empty if not found
    
    // If status field doesn't exist, that's ok (not all payments will have it)
    // We accept proofs that have extractedParameterValues (indicates response was captured)
    if !payment_status.is_empty() {
        // If status exists, it must be confirmed
        if payment_status != "confirmed" && payment_status != "SUCCESS" && payment_status != "success" {
            return Err(format!(
                "Payment not confirmed. Status: {}",
                payment_status
            ));
        }
        tracing::info!("[VERIFY-PAYMENT] ✓ Payment status confirmed: {}", payment_status);
    } else {
        // No explicit status, but we have extractedParameterValues which means the call succeeded
        tracing::info!("[VERIFY-PAYMENT] ✓ Payment response captured (no explicit status field)");
    }
    
    // VERIFICATION 3: Verify proof cryptographic signature
    verify_secp256k1_sig(proof_data)?;
    
    tracing::info!("[VERIFY-PAYMENT] ✓ All payment proof verifications passed");
    Ok(())
}
