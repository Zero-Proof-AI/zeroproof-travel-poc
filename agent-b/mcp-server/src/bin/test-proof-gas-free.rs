use std::fs;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();

    println!("\n========================================");
    println!("🧪 Testing Proof Verification (Gas-Free)");
    println!("========================================");
    println!("📍 Mode: Call (no transaction, no private key needed)\n");

    // Load the same proof file
    let proof_file = "/home/revolution/zkfetch-wrapper/proof-structure.json";
    let proof_json = match fs::read_to_string(proof_file) {
        Ok(content) => {
            println!("✅ Loaded proof from: {}", proof_file);
            content
        }
        Err(e) => {
            eprintln!("❌ Failed to read proof file: {}", e);
            std::process::exit(1);
        }
    };

    let raw_proof: serde_json::Value = match serde_json::from_str(&proof_json) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("❌ Failed to parse proof JSON: {}", e);
            std::process::exit(1);
        }
    };

    println!("✅ Raw proof parsed successfully");
    
    // Transform raw proof to onchain format
    println!("📝 Transforming raw proof to onchain format...");
    let proof_data = match transform_raw_to_onchain(&raw_proof) {
        Ok(data) => {
            println!("✅ Proof transformed successfully");
            data
        }
        Err(e) => {
            eprintln!("❌ Failed to transform proof: {}", e);
            std::process::exit(1);
        }
    };
    
    // Call the gas-free verification function
    println!("\n📋 Starting gas-free proof verification...\n");

    match verify_proof_gas_free(&proof_data).await {
        Ok(result) => {
            println!("\n✅ VERIFICATION PASSED!");
            println!("========================================");
            println!("📊 Result Details:");
            println!("  Proof ID: {}", result.identifier);
            println!("  Owner: {}", result.owner);
            println!("  Timestamp: {}", result.timestamp);
            println!("  Epoch: {}", result.epoch);
            println!("  Signatures Verified: {}", result.signatures_count);
            println!("========================================");
            println!("🎉 No gas spent, no private key needed!");
            println!("========================================\n");
            std::process::exit(0);
        }
        Err(e) => {
            println!("\n❌ VERIFICATION FAILED!");
            println!("========================================");
            println!("Error: {}", e);
            println!("========================================\n");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Clone)]
struct VerificationResult {
    identifier: String,
    owner: String,
    timestamp: u32,
    epoch: u32,
    signatures_count: usize,
}

/// Verify proof without sending transaction (gas-free, no private key needed)
/// This calls the contract with static call, just reads the result
async fn verify_proof_gas_free(
    proof_data: &serde_json::Value,
) -> Result<VerificationResult, String> {
    use ethers::prelude::*;
    use ethers::abi::Token;
    use ethers::types::transaction::eip2718::TypedTransaction;
    use hex;

    // Get configuration from environment
    let rpc_url = std::env::var("SEPOLIA_RPC_URL")
        .unwrap_or_else(|_| "https://sepolia.sepolia.io".to_string());

    let contract_address_str = std::env::var("RECLAIM_ADDRESS")
        .unwrap_or_else(|_| "0xAe94FB09711e1c6B057853a515483792d8e474d0".to_string());

    tracing::info!("[VERIFY-GAS-FREE] Calling contract (no transaction): {}", contract_address_str);

    // Step 1: Connect to RPC provider (no signer needed!)
    tracing::debug!("[VERIFY-GAS-FREE] Connecting to RPC provider: {}", rpc_url);
    let provider = Provider::<Http>::try_from(&rpc_url)
        .map_err(|e| format!("Failed to connect to RPC provider: {}", e))?;

    // Step 2: Get chain ID
    let network = provider.get_chainid()
        .await
        .map_err(|e| format!("Failed to get chain ID: {}", e))?;
    tracing::debug!("[VERIFY-GAS-FREE] Connected to chain ID: {}", network);

    // Step 3: Parse contract address
    let contract_addr: Address = contract_address_str.parse()
        .map_err(|e| format!("Invalid contract address: {}", e))?;
    tracing::debug!("[VERIFY-GAS-FREE] Contract address parsed: {}", contract_addr);

    // Step 4: Verify contract exists
    let code = provider.get_code(contract_addr, None)
        .await
        .map_err(|e| format!("Failed to check contract deployment: {}", e))?;

    if code.is_empty() {
        return Err(format!("Contract not deployed at address: {}", contract_addr));
    }
    tracing::debug!("[VERIFY-GAS-FREE] Contract verified as deployed");

    // Step 5: Extract onchainProof
    let onchain_proof_value = if let Some(op) = proof_data.get("onchainProof") {
        tracing::debug!("[VERIFY-GAS-FREE] Found onchainProof at top level");
        op.clone()
    } else if proof_data.get("claimInfo").is_some() {
        tracing::debug!("[VERIFY-GAS-FREE] Proof is already in onchain format");
        proof_data.clone()
    } else {
        return Err("Missing onchainProof or claimInfo in proof_data".to_string());
    };

    // Extract ClaimInfo fields
    let claim_info = onchain_proof_value.get("claimInfo").ok_or("Missing claimInfo")?;
    let provider_name = claim_info.get("provider").and_then(|v| v.as_str()).ok_or("Missing provider")?;
    let parameters = claim_info.get("parameters").and_then(|v| v.as_str()).ok_or("Missing parameters")?;
    let context = claim_info.get("context").and_then(|v| v.as_str()).ok_or("Missing context")?;

    tracing::debug!("[VERIFY-GAS-FREE] ClaimInfo loaded");

    // Extract SignedClaim data
    let signed_claim = onchain_proof_value.get("signedClaim").ok_or("Missing signedClaim")?;
    let claim_data = signed_claim.get("claim").ok_or("Missing claim in signedClaim")?;
    let identifier_str = claim_data.get("identifier").and_then(|v| v.as_str()).ok_or("Missing identifier")?;
    let owner_str = claim_data.get("owner").and_then(|v| v.as_str()).ok_or("Missing owner")?;

    tracing::info!("[VERIFY-GAS-FREE] Identifier: {}", identifier_str);
    tracing::info!("[VERIFY-GAS-FREE] Owner: {}", owner_str);

    // Parse identifier
    let identifier_h256: H256 = identifier_str.parse()
        .map_err(|e| format!("Invalid identifier hex: {}", e))?;

    // Parse owner address
    let owner_addr: Address = owner_str.parse()
        .map_err(|e| format!("Invalid owner address: {}", e))?;

    let timestamp = claim_data.get("timestampS").and_then(|v| v.as_u64()).ok_or("Missing timestampS")? as u32;
    let epoch = claim_data.get("epoch").and_then(|v| v.as_u64()).ok_or("Missing epoch")? as u32;

    tracing::debug!("[VERIFY-GAS-FREE] Timestamp: {}, Epoch: {}", timestamp, epoch);

    // Build ClaimInfo tuple token: (string, string, string)
    let claim_info_token = Token::Tuple(vec![
        Token::String(provider_name.to_string()),
        Token::String(parameters.to_string()),
        Token::String(context.to_string()),
    ]);

    // Build CompleteClaimData tuple token: (bytes32, address, uint32, uint32)
    let complete_claim_token = Token::Tuple(vec![
        Token::FixedBytes(identifier_h256.as_bytes().to_vec()),
        Token::Address(owner_addr),
        Token::Uint(U256::from(timestamp)),
        Token::Uint(U256::from(epoch)),
    ]);

    // Extract signatures array
    let signatures = signed_claim.get("signatures").and_then(|v| v.as_array()).ok_or("Missing signatures")?;
    let signatures_count = signatures.len();
    
    tracing::info!("[VERIFY-GAS-FREE] Processing {} signatures", signatures_count);

    let sig_tokens: Result<Vec<Token>, String> = signatures
        .iter()
        .map(|sig| {
            let sig_str = sig.as_str().ok_or_else(|| "Signature is not a string".to_string())?;
            let sig_bytes = hex::decode(sig_str.trim_start_matches("0x"))
                .map_err(|e| format!("Failed to decode signature: {}", e))?;
            Ok(Token::Bytes(sig_bytes))
        })
        .collect();

    let sig_tokens = sig_tokens?;

    // Build SignedClaim tuple token: (CompleteClaimData, bytes[])
    let signed_claim_token = Token::Tuple(vec![
        complete_claim_token,
        Token::Array(sig_tokens),
    ]);

    // Build Proof tuple token: (ClaimInfo, SignedClaim)
    let proof_token = Token::Tuple(vec![
        claim_info_token,
        signed_claim_token,
    ]);

    // Encode the function call: verifyProof((claimInfo), (completeClaimData, signatures[]))
    let function_selector = "verifyProof(((string,string,string),((bytes32,address,uint32,uint32),bytes[])))";
    
    let sig_hash = ethers::utils::keccak256(function_selector.as_bytes());
    let function_id = &sig_hash[0..4];

    tracing::debug!("[VERIFY-GAS-FREE] Function selector: 0x{}", hex::encode(function_id));

    // Encode parameters
    let encoded_params = ethers::abi::encode(&vec![proof_token]);
    let call_data = [function_id, &encoded_params].concat();

    tracing::debug!("[VERIFY-GAS-FREE] Call data size: {} bytes", call_data.len());

    // Step 6: Make the static call (no transaction, gas-free)
    tracing::info!("[VERIFY-GAS-FREE] Making static call to verifyProof...");

    let tx_request = TransactionRequest::new()
        .to(contract_addr)
        .data(call_data);
    
    let typed_tx = TypedTransaction::Legacy(tx_request);

    let result = provider
        .call(&typed_tx, None)
        .await
        .map_err(|e| format!("Contract call failed: {}", e))?;

    tracing::debug!("[VERIFY-GAS-FREE] Call result: {} bytes", result.len());

    // Parse boolean return value
    if result.len() >= 32 {
        let last_byte = result[31];
        if last_byte == 1 {
            tracing::info!("[VERIFY-GAS-FREE] ✓ Proof verification returned TRUE");
            
            Ok(VerificationResult {
                identifier: identifier_str.to_string(),
                owner: owner_str.to_string(),
                timestamp,
                epoch,
                signatures_count,
            })
        } else {
            Err("Contract verification returned FALSE".to_string())
        }
    } else {
        Err("Invalid return value from contract".to_string())
    }
}

/// Transform raw proof format to onchain format
fn transform_raw_to_onchain(raw_proof: &serde_json::Value) -> Result<serde_json::Value, String> {
    // Extract claimData
    let claim_data = raw_proof.get("claimData").ok_or("Missing claimData")?;
    let provider = claim_data.get("provider").and_then(|v| v.as_str()).ok_or("Missing provider")?;
    let parameters = claim_data.get("parameters").and_then(|v| v.as_str()).ok_or("Missing parameters")?;
    let context = claim_data.get("context").and_then(|v| v.as_str()).ok_or("Missing context")?;
    
    // Extract claim fields
    let owner = claim_data.get("owner").and_then(|v| v.as_str()).ok_or("Missing owner")?;
    let timestamp_s = claim_data.get("timestampS").and_then(|v| v.as_u64()).ok_or("Missing timestampS")?;
    let epoch = claim_data.get("epoch").and_then(|v| v.as_u64()).ok_or("Missing epoch")?;
    let identifier = raw_proof.get("identifier").and_then(|v| v.as_str()).ok_or("Missing identifier")?;
    
    // Extract signatures
    let signatures = raw_proof.get("signatures").and_then(|v| v.as_array()).ok_or("Missing signatures")?;
    let sig_array: Vec<serde_json::Value> = signatures.iter().cloned().collect();
    
    // Build onchain format
    let onchain_proof = serde_json::json!({
        "claimInfo": {
            "provider": provider,
            "parameters": parameters,
            "context": context
        },
        "signedClaim": {
            "claim": {
                "identifier": identifier,
                "owner": owner,
                "timestampS": timestamp_s,
                "epoch": epoch
            },
            "signatures": sig_array
        }
    });
    
    Ok(serde_json::json!({
        "onchainProof": onchain_proof
    }))
}
