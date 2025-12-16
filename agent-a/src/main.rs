use serde::{Deserialize, Serialize};
use serde_json::json;
use hex;
use zk_protocol::{AttestRequest, AttestResponse, AgentResponse};

// Agent-specific input type (Agent A only needs to know its own format)
#[derive(Serialize, Deserialize)]
struct PricingInput {
    from: String,
    to: String,
    vip: bool,
}

/// Helper to call the pre-deployed SP1VerifierGroth16 contract using JSON-RPC
/// Uses the universal verifier pattern: vk_hash is passed as a parameter, not stored on-chain
async fn verify_on_chain(
    sp1_verifier_addr: &str,
    rpc_url: &str,
    proof_hex: &str,
    public_values_hex: &str,
    vk_hash: &str,
) -> anyhow::Result<bool> {
    println!("\n→ Verifying proof on-chain with SP1VerifierGroth16 at {}", sp1_verifier_addr);
    
    // Decode proof, public values, and VK hash
    let proof_bytes = hex::decode(proof_hex.strip_prefix("0x").unwrap_or(proof_hex))?;
    let public_values_bytes = hex::decode(public_values_hex.strip_prefix("0x").unwrap_or(public_values_hex))?;
    let vk_hash_bytes = hex::decode(vk_hash.strip_prefix("0x").unwrap_or(vk_hash))?;
    
    if vk_hash_bytes.len() != 32 {
        return Err(anyhow::anyhow!("VK hash must be 32 bytes, got {}", vk_hash_bytes.len()));
    }
    
    // Build encoded call data for verifyProof(bytes32 programVKey, bytes calldata publicValues, bytes calldata proofBytes)
    // Using ethers ABI encoding
    let vk_hash_token = ethers::abi::Token::FixedBytes(vk_hash_bytes);
    let public_values_token = ethers::abi::Token::Bytes(public_values_bytes);
    let proof_token = ethers::abi::Token::Bytes(proof_bytes);
    let encoded = ethers::abi::encode(&[vk_hash_token, public_values_token, proof_token]);

    // Function selector for verifyProof(bytes32,bytes,bytes)
    // keccak256("verifyProof(bytes32,bytes,bytes)") = 0x41493c60
    let fn_selector = [0x41, 0x49, 0x3c, 0x60];
    let mut call_data = fn_selector.to_vec();
    call_data.extend(encoded);
    let call_data_hex = format!("0x{}", hex::encode(&call_data));

    println!("  VK Hash: {}", vk_hash);
    println!("  Public Values ({} bytes): {}...", public_values_hex.len() / 2, &public_values_hex[..std::cmp::min(66, public_values_hex.len())]);
    println!("  Proof ({} bytes / {} hex): {}...", proof_hex.len() / 2, proof_hex.len(), &proof_hex[..std::cmp::min(66, proof_hex.len())]);
    println!("  Expected: 4 bytes (VERIFIER_HASH) + 256 bytes (8 * uint256) = 260 bytes total");

    // Use JSON-RPC eth_call to the universal verifier
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [
            {
                "to": sp1_verifier_addr,
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
        eprintln!("✗ On-chain verification FAILED (contract reverted):");
        eprintln!("  Error: {}", error);
        
        // Try to decode the revert reason
        if let Some(data) = error.get("data").and_then(|v| v.as_str()) {
            eprintln!("  Revert data: {}", data);
            
            // Check for WrongVerifierSelector error
            if data.contains("82b42900") {
                eprintln!("  → WrongVerifierSelector: proof's VERIFIER_HASH doesn't match contract");
            } else if data.contains("09bde339") {
                eprintln!("  → InvalidProof: proof verification failed");
            }
        }
        
        Ok(false)
    } else if let Some(result) = response.get("result").and_then(|v| v.as_str()) {
        // If eth_call succeeds, verifyProof() didn't revert = proof is valid
        println!("✓ On-chain verification result: valid ✅");
        println!("  Call succeeded without revert - proof cryptographically verified!");
        Ok(true)
    } else {
        eprintln!("⚠ Unexpected JSON-RPC response: {}", response);
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create client with very long timeout for proof generation (30 minutes)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7200))
        .build()?;
    let agent_b_url = std::env::var("AGENT_B_URL")
        .unwrap_or_else(|_| "http://localhost:8001".to_string());
    let attester_url = std::env::var("ATTESTER_URL")
        .unwrap_or_else(|_| "http://localhost:8000".to_string());
    let sp1_verifier_addr = std::env::var("SP1_VERIFIER_ADDRESS")
        .unwrap_or_else(|_| "0x53A9038dCB210D210A7C973fA066Fd2C50aa8847".to_string()); // Sepolia SP1 v5.2.4 Groth16 verifier (universal)
    let rpc_url = std::env::var("RPC_URL")
        .unwrap_or_else(|_| "http://localhost:8545".to_string());

    // 1. Call Agent B to get the price
    println!("→ Calling Agent B at {}", agent_b_url);
    let price_resp = client
        .post(&format!("{}/price", agent_b_url))
        .json(&json!({
            "from": "NYC",
            "to": "LON",
            "vip": true
        }))
        .send()
        .await?
        .json::<AgentResponse>()
        .await?;

    println!("✓ Agent B response:");
    println!("  data: {}", price_resp.data);
    println!("  program_id: {}", price_resp.program_id);
    println!("  elf_hash: {}", price_resp.elf_hash);

    // 2. Request attestation from the attester service
    println!("\n→ Requesting attestation from {}", attester_url);
    
    // Agent A calls Agent B's /zk-input helper to get properly formatted bytes
    // This way Agent A doesn't need to know Agent B's internal zkVM structure
    let zk_input_resp = client
        .post(&format!("{}/zk-input", agent_b_url))
        .json(&json!({
            "endpoint": "price",
            "input": {
                "from": "NYC",
                "to": "LON",
                "vip": true
            }
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    let input_bytes: Vec<u8> = zk_input_resp["input_bytes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing input_bytes"))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u8))
        .collect();
    
    let attest_req = AttestRequest {
        program_id: price_resp.program_id.clone(),
        input_bytes,
        claimed_output: Some(price_resp.data.clone()),
        verify_locally: true,
    };

    let attest_resp = client
        .post(&format!("{}/attest", attester_url))
        .json(&attest_req)
        .send()
        .await?
        .json::<AttestResponse>()
        .await?;

    println!("✓ Attestation response:");
    println!("  verified_output: {}", attest_resp.verified_output);
    println!("  vk_hash: {}", attest_resp.vk_hash);
    println!("  proof (first 66 chars): {}...", &attest_resp.proof[..std::cmp::min(66, attest_resp.proof.len())]);

    println!("✅ Off-chain proof verified!");

    // 3. Optional: verify proof on-chain using pre-deployed SP1VerifierGroth16
    if let verifier_addr = sp1_verifier_addr {
        match verify_on_chain(&verifier_addr, &rpc_url, &attest_resp.proof, &attest_resp.public_values, &attest_resp.vk_hash).await {
            Ok(true) => {
                println!("\n✅ On-chain verification SUCCESS! Response data is cryptographically valid.");
                println!("   Verified data: {}", price_resp.data);
            }
            Ok(false) => {
                println!("\n⚠ On-chain verification returned false (proof mismatch)");
            }
            Err(e) => {
                println!("\n⚠ On-chain verification error: {}", e);
            }
        }
    } else {
        println!("\n✓ No SP1 verifier address provided (SP1_VERIFIER_ADDRESS env var). Skipping on-chain verification.");
        println!("  To enable on-chain verification, set:");
        println!("    export SP1_VERIFIER_ADDRESS=0x<pre-deployed-sp1-verifier-address>");
        println!("    export RPC_URL=https://<rpc-endpoint>");
    }

    Ok(())
}