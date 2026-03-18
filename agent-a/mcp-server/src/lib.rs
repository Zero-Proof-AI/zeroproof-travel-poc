/// Agent A MCP Server Library
/// 
/// Exposes ZK proof operations as MCP tools:
/// - verify_on_chain: Verify proofs on Sepolia testnet
/// - request_attestation: Request attestation from attester service
/// - format_zk_input: Format input for zkVM
/// - call_agent_b: Call Agent B pricing/booking endpoints

use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use anyhow::Result;

// Re-export from zk-protocol
pub use zk_protocol::{AttestRequest, AttestResponse, AgentResponse};

/// Pricing input for Agent B
#[derive(Debug, Serialize, Deserialize, JsonSchema, Clone)]
pub struct PricingInput {
    /// Source location
    pub from: String,
    /// Destination location
    pub to: String,
    /// VIP status
    pub vip: bool,
}

/// Response from pricing service
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PricingResponse {
    /// Calculated price
    pub price: f64,
    /// Program ID for attestation
    pub program_id: String,
    /// ELF hash for verification
    pub elf_hash: String,
}

/// On-chain verification result
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct VerificationResult {
    /// Whether verification succeeded
    pub verified: bool,
    /// Optional error message
    pub error: Option<String>,
    /// Details from contract call
    pub details: Option<String>,
}

/// Attestation request parameters
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AttestationParams {
    /// Program ID from Agent B
    pub program_id: String,
    /// Input bytes (as hex string or array)
    pub input_hex: String,
    /// Claimed output value
    pub claimed_output: String,
    /// Whether to verify locally
    pub verify_locally: bool,
}

/// ZK input formatting parameters
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ZkInputParams {
    /// Agent B endpoint (e.g., "price", "booking")
    pub endpoint: String,
    /// Input data as JSON
    pub input: serde_json::Value,
}

/// ZK input formatting result
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ZkInputResult {
    /// Input formatted as bytes (hex string)
    pub input_bytes: String,
    /// Input as array of u8 for verification
    pub input_array: Vec<u8>,
}

/// Verifies proof on-chain with Sepolia ZeroProof contract
pub async fn verify_on_chain(
    zeroproof_addr: &str,
    rpc_url: &str,
    proof_hex: &str,
    public_values_hex: &str,
    vk_hash: &str,
) -> Result<bool> {
    tracing::info!("→ Verifying proof on-chain with ZeroProof at {}", zeroproof_addr);
    
    // Decode proof, public values, and VK hash
    let proof_bytes = hex::decode(proof_hex.strip_prefix("0x").unwrap_or(proof_hex))?;
    let public_values_bytes = hex::decode(public_values_hex.strip_prefix("0x").unwrap_or(public_values_hex))?;
    let vk_hash_bytes = hex::decode(vk_hash.strip_prefix("0x").unwrap_or(vk_hash))?;
    
    if vk_hash_bytes.len() != 32 {
        return Err(anyhow::anyhow!("VK hash must be 32 bytes, got {}", vk_hash_bytes.len()));
    }
    
    // Build ZeroProof.verifyProof(bytes32 proofType, bytes calldata proof, Claim calldata claim)
    // For SP1 proofs: proofType = keccak256("sp1-zkvm")
    let proof_type = ethers::core::utils::keccak256(b"sp1-zkvm");
    
    // SP1 proof format: encode(vkey, publicValues, proofBytes)
    let sp1_proof = {
        let vk_token = ethers::abi::Token::FixedBytes(vk_hash_bytes.clone());
        let pv_token = ethers::abi::Token::Bytes(public_values_bytes.clone());
        let proof_token = ethers::abi::Token::Bytes(proof_bytes.clone());
        ethers::abi::encode(&[vk_token, pv_token, proof_token])
    };
    
    // Claim structure: (address agent, bytes32 claimType, bytes publicData, bytes32 dataHash)
    let claim = {
        let agent = ethers::abi::Token::Address(ethers::types::Address::zero());
        let claim_type = ethers::abi::Token::FixedBytes(ethers::core::utils::keccak256(b"pricing").to_vec());
        let public_data = ethers::abi::Token::Bytes(public_values_bytes.clone());
        let data_hash = ethers::abi::Token::FixedBytes(ethers::core::utils::keccak256(&public_values_bytes).to_vec());
        ethers::abi::Token::Tuple(vec![agent, claim_type, public_data, data_hash])
    };
    
    // Encode function call: verifyProof(bytes32,bytes,(address,bytes32,bytes,bytes32))
    let proof_type_token = ethers::abi::Token::FixedBytes(proof_type.to_vec());
    let proof_token = ethers::abi::Token::Bytes(sp1_proof);
    let encoded = ethers::abi::encode(&[proof_type_token, proof_token, claim]);

    // Function selector for verifyProof(bytes32,bytes,(address,bytes32,bytes,bytes32))
    let fn_selector = &ethers::core::utils::keccak256(b"verifyProof(bytes32,bytes,(address,bytes32,bytes,bytes32))")[..4];
    let mut call_data = fn_selector.to_vec();
    call_data.extend(encoded);
    let call_data_hex = format!("0x{}", hex::encode(&call_data));

    tracing::debug!("Proof Type: sp1-zkvm ({})", hex::encode(&proof_type));
    tracing::debug!("VK Hash: {}", vk_hash);
    tracing::debug!("Public Values ({} bytes)", public_values_hex.len() / 2);

    // Use JSON-RPC eth_call to ZeroProof contract
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [
            {
                "to": zeroproof_addr,
                "data": call_data_hex,
            },
            "latest"
        ],
        "id": 1,
    });

    let client = reqwest::Client::new();
    let response: serde_json::Value = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await?
        .json()
        .await?;

    if let Some(error) = response.get("error") {
        tracing::error!("✗ On-chain verification FAILED (contract reverted): {}", error);
        Ok(false)
    } else if response.get("result").and_then(|v| v.as_str()).is_some() {
        // If eth_call succeeds, verifyProof() didn't revert = proof is valid
        tracing::info!("✓ On-chain verification result: valid");
        Ok(true)
    } else {
        tracing::warn!("⚠ Unexpected JSON-RPC response: {}", response);
        Ok(false)
    }
}

/// Call Agent B to get pricing and program info
pub async fn get_ticket_price(
    agent_b_url: &str,
    input: &PricingInput,
) -> Result<PricingResponse> {
    tracing::info!("→ Calling Agent B at {}", agent_b_url);
    
    let client = reqwest::Client::new();
    let response_json = client
        .post(&format!("{}/price", agent_b_url))
        .json(&serde_json::json!({
            "from": input.from,
            "to": input.to,
            "vip": input.vip
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    // Extract fields directly from response JSON
    let price = response_json
        .get("price")
        .and_then(|p| p.as_f64())
        .unwrap_or(0.0);
    
    let program_id = response_json
        .get("program_id")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    
    let elf_hash = response_json
        .get("elf_hash")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();

    tracing::info!("✓ Agent B response: price={}, program_id={}", price, program_id);

    Ok(PricingResponse {
        price,
        program_id,
        elf_hash,
    })
}

/// Get ZK input formatting from Agent B
pub async fn format_zk_input(
    agent_b_url: &str,
    endpoint: &str,
    input: &serde_json::Value,
) -> Result<ZkInputResult> {
    tracing::info!("→ Getting ZK input format from Agent B");
    
    let client = reqwest::Client::new();
    let response = client
        .post(&format!("{}/zk-input", agent_b_url))
        .json(&serde_json::json!({
            "endpoint": endpoint,
            "input": input
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    let input_array: Vec<u8> = response["input_bytes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing input_bytes in response"))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u8))
        .collect();

    let input_hex = format!("0x{}", hex::encode(&input_array));
    
    tracing::info!("✓ ZK input formatted: {} bytes", input_array.len());

    Ok(ZkInputResult {
        input_bytes: input_hex,
        input_array,
    })
}

/// Request attestation from attester service
pub async fn request_attestation(
    attester_url: &str,
    program_id: &str,
    input_bytes: Vec<u8>,
    claimed_output: Option<serde_json::Value>,
    verify_locally: bool,
) -> Result<AttestResponse> {
    tracing::info!("→ Requesting attestation from {}", attester_url);
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7200))
        .build()?;

    let request = AttestRequest {
        program_id: program_id.to_string(),
        input_bytes,
        stdin_items: vec![],
        claimed_output,
        verify_locally,
    };

    let response = client
        .post(&format!("{}/attest", attester_url))
        .json(&request)
        .send()
        .await?
        .json::<AttestResponse>()
        .await?;

    tracing::info!("✓ Attestation response: verified_output={}", response.verified_output);

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_input_schema() {
        let input = PricingInput {
            from: "NYC".to_string(),
            to: "LON".to_string(),
            vip: true,
        };
        let schema = schemars::schema_for!(PricingInput);
        assert!(schema.schema.object.is_some());
    }
}
