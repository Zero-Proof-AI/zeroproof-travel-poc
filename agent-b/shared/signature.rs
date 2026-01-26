use secp256k1::{Secp256k1, ecdsa::RecoveryId};
use sha3::{Keccak256, Digest};

/// Verifies a proof's cryptographic signature
/// 
/// Dispatches to either local SDK verification or on-chain contract verification
/// based on the `onchain` parameter. These are mutually exclusive approaches.
pub async fn verify_secp256k1_sig(
    proof_data: &serde_json::Value,
    onchain: bool,
    no_gas: bool,
) -> Result<(), String> {
    if onchain {
        if no_gas {
            verify_proof_gas_free(proof_data).await
        } else {
            verify_onchain_sig(proof_data).await
        }
    } else {
        verify_sdk_sig(proof_data)
    }
}

/// Verifies a proof gas-free using static call (no private key, no gas cost)
/// 
/// This calls the contract's verifyProof function without sending a transaction.
/// Returns whether the proof is valid without spending gas.
/// 
/// This requires:
/// - SEPOLIA_RPC_URL: RPC endpoint (default: https://sepolia.sepolia.io)
/// - RECLAIM_ADDRESS: Contract address (default: 0xAe94FB09711e1c6B057853a515483792d8e474d0)
pub async fn verify_proof_gas_free(
    proof_data: &serde_json::Value,
) -> Result<(), String> {
    use ethers::prelude::*;
    use ethers::abi::Token;
    use ethers::types::transaction::eip2718::TypedTransaction;
    use hex;

    // Get configuration from environment
    let rpc_url = std::env::var("SEPOLIA_RPC_URL")
        .unwrap_or_else(|_| "https://sepolia.sepolia.io".to_string());

    let contract_address_str = std::env::var("RECLAIM_ADDRESS")
        .unwrap_or_else(|_| "0xAe94FB09711e1c6B057853a515483792d8e474d0".to_string());

    tracing::info!("[VERIFY-PROOF-GAS-FREE] Calling contract (no transaction): {}", contract_address_str);

    // Step 1: Connect to RPC provider (no signer needed!)
    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Connecting to RPC provider: {}", rpc_url);
    let provider = Provider::<Http>::try_from(&rpc_url)
        .map_err(|e| format!("Failed to connect to RPC provider: {}", e))?;

    // Step 2: Get chain ID
    let network = provider.get_chainid()
        .await
        .map_err(|e| format!("Failed to get chain ID: {}", e))?;
    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Connected to chain ID: {}", network);

    // Step 3: Parse contract address
    let contract_addr: Address = contract_address_str.parse()
        .map_err(|e| format!("Invalid contract address: {}", e))?;
    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Contract address parsed: {}", contract_addr);

    // Step 4: Verify contract exists
    let code = provider.get_code(contract_addr, None)
        .await
        .map_err(|e| format!("Failed to check contract deployment: {}", e))?;

    if code.is_empty() {
        return Err(format!("Contract not deployed at address: {}", contract_addr));
    }
    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Contract verified as deployed");

    // Step 5: Extract onchainProof
    let onchain_proof_value = if let Some(op) = proof_data.get("onchainProof") {
        tracing::debug!("[VERIFY-PROOF-GAS-FREE] Found onchainProof at top level");
        op.clone()
    } else if proof_data.get("claimInfo").is_some() {
        tracing::debug!("[VERIFY-PROOF-GAS-FREE] Proof is already in onchain format");
        proof_data.clone()
    } else {
        return Err("Missing onchainProof or claimInfo in proof_data".to_string());
    };

    // Extract ClaimInfo fields
    let claim_info = onchain_proof_value.get("claimInfo").ok_or("Missing claimInfo")?;
    let provider_name = claim_info.get("provider").and_then(|v| v.as_str()).ok_or("Missing provider")?;
    let parameters = claim_info.get("parameters").and_then(|v| v.as_str()).ok_or("Missing parameters")?;
    let context = claim_info.get("context").and_then(|v| v.as_str()).ok_or("Missing context")?;

    tracing::debug!("[VERIFY-PROOF-GAS-FREE] ClaimInfo loaded");

    // Extract SignedClaim data
    let signed_claim = onchain_proof_value.get("signedClaim").ok_or("Missing signedClaim")?;
    let claim_data = signed_claim.get("claim").ok_or("Missing claim in signedClaim")?;
    let identifier_str = claim_data.get("identifier").and_then(|v| v.as_str()).ok_or("Missing identifier")?;
    let owner_str = claim_data.get("owner").and_then(|v| v.as_str()).ok_or("Missing owner")?;

    tracing::info!("[VERIFY-PROOF-GAS-FREE] Identifier: {}", identifier_str);
    tracing::info!("[VERIFY-PROOF-GAS-FREE] Owner: {}", owner_str);

    // Parse identifier
    let identifier_h256: H256 = identifier_str.parse()
        .map_err(|e| format!("Invalid identifier hex: {}", e))?;

    // Parse owner address
    let owner_addr: Address = owner_str.parse()
        .map_err(|e| format!("Invalid owner address: {}", e))?;

    let timestamp = claim_data.get("timestampS").and_then(|v| v.as_u64()).ok_or("Missing timestampS")? as u32;
    let epoch = claim_data.get("epoch").and_then(|v| v.as_u64()).ok_or("Missing epoch")? as u32;

    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Timestamp: {}, Epoch: {}", timestamp, epoch);

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

    tracing::info!("[VERIFY-PROOF-GAS-FREE] Processing {} signatures", signatures.len());

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

    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Function selector: 0x{}", hex::encode(function_id));

    // Encode parameters
    let encoded_params = ethers::abi::encode(&vec![proof_token]);
    
    // Build call data correctly: function selector + encoded parameters
    let mut call_data = function_id.to_vec();
    call_data.extend_from_slice(&encoded_params);

    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Call data size: {} bytes", call_data.len());
    tracing::info!("[VERIFY-PROOF-GAS-FREE] Making static call to verifyProof...");

    // Make the static call (no transaction, gas-free)
    let tx_request = TransactionRequest::new()
        .to(contract_addr)
        .data(call_data);
    
    let typed_tx = TypedTransaction::Legacy(tx_request);

    let result = provider
        .call(&typed_tx, None)
        .await
        .map_err(|e| format!("Contract call failed: {}", e))?;

    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Call result: {} bytes", result.len());
    tracing::debug!("[VERIFY-PROOF-GAS-FREE] Call result hex: 0x{}", hex::encode(&result));

    // Parse boolean return value
    if result.len() >= 32 {
        let last_byte = result[31];
        tracing::debug!("[VERIFY-PROOF-GAS-FREE] Last byte of result: {}", last_byte);
        if last_byte == 1 {
            tracing::info!("[VERIFY-PROOF-GAS-FREE] ✓ Proof verification returned TRUE");
            Ok(())
        } else {
            Err(format!("Contract verification returned FALSE (result[31]={})", last_byte))
        }
    } else {
        Err(format!("Invalid return value from contract (len={})", result.len()))
    }
}

/// Verifies a proof's cryptographic signature locally using SDK approach
/// 
/// This implements the Reclaim SDK protocol's signature verification:
/// 1. Extracts claim data, signatures, and witnesses from proof_data
/// 2. Reconstructs the signed message in format: identifier\nowner\ntimestamp\nepoch
/// 3. Adds Ethereum prefix: "\x19Ethereum Signed Message:\n<length>"
/// 4. Hashes with keccak256
/// 5. Recovers the signer from the signature via secp256k1
/// 6. Compares recovered signer with expected witness
/// 
/// NOTE: This is a pure synchronous function - no async I/O operations
pub fn verify_sdk_sig(
    proof_data: &serde_json::Value,
) -> Result<(), String> {
    // Extract claim data and signatures from proof_data
    let claim_data = proof_data
        .get("proof")
        .and_then(|p| p.get("claimData"))
        .ok_or_else(|| "Proof missing claimData for signature verification".to_string())?;
    
    let signatures_array = proof_data
        .get("proof")
        .and_then(|p| p.get("signatures"))
        .and_then(|sigs| sigs.as_array())
        .ok_or_else(|| "Proof missing signatures array".to_string())?;
    
    let signatures: Vec<String> = signatures_array
        .iter()
        .filter_map(|s| s.as_str().map(|s| s.to_string()))
        .collect();
    
    if signatures.is_empty() {
        return Err("No valid signatures in proof".to_string());
    }
    
    // Get witnesses from proof for signature verification
    let witnesses_array = proof_data
        .get("proof")
        .and_then(|p| p.get("witnesses"))
        .and_then(|w| w.as_array())
        .ok_or_else(|| "Proof missing witnesses array".to_string())?;
    
    if witnesses_array.is_empty() {
        return Err("No witnesses found in proof".to_string());
    }
    
    // Extract claim data fields
    let identifier = claim_data
        .get("identifier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing identifier in claim data".to_string())?;
    
    let owner = claim_data
        .get("owner")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing owner in claim data".to_string())?;
    
    let timestamp_s = claim_data
        .get("timestampS")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing timestampS in claim data".to_string())?;
    
    let epoch = claim_data
        .get("epoch")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "Missing epoch in claim data".to_string())?;
    
    // Get first signature
    let signature_str = signatures
        .first()
        .ok_or_else(|| "No signatures found in proof".to_string())?;
    
    // Reconstruct the message that was signed (Reclaim SDK format)
    let message = format!(
        "{}\n{}\n{}\n{}",
        identifier.to_lowercase(),
        owner.to_lowercase(),
        timestamp_s,
        epoch
    );
    
    tracing::debug!("[VERIFY-SIG] Message to verify: {:?}", message);
    tracing::debug!("[VERIFY-SIG] Signature: {}", signature_str);
    
    // Add Ethereum message prefix and hash
    let prefixed_message = format!(
        "\x19Ethereum Signed Message:\n{}{}",
        message.len(),
        message
    );
    
    let mut hasher = Keccak256::new();
    hasher.update(prefixed_message.as_bytes());
    let message_hash = hasher.finalize();
    tracing::debug!("[VERIFY-SIG] Message hash: 0x{}", hex::encode(&message_hash[..]));
    
    // Parse the signature
    let signature_bytes = hex::decode(
        signature_str.trim_start_matches("0x")
    ).map_err(|e| format!("Failed to decode signature hex: {}", e))?;
    
    if signature_bytes.len() != 65 {
        return Err(format!(
            "Invalid signature length: {} (expected 65)",
            signature_bytes.len()
        ));
    }
    
    // Convert signature to secp256k1 format
    // Ethereum signatures are (v, r, s) where v is 27/28 (or 0/1 in some formats)
    let v = signature_bytes[64];
    let recovery_id = if v >= 27 { v - 27 } else { v };
    
    // Get the r and s components
    let r = &signature_bytes[0..32];
    let s = &signature_bytes[32..64];
    
    // Create secp256k1 signature
    let secp = Secp256k1::new();
    let recovery_id = RecoveryId::from_i32(recovery_id as i32)
        .map_err(|e| format!("Invalid recovery ID: {}", e))?;
    
    // Create the signature
    let sig = secp256k1::ecdsa::RecoverableSignature::from_compact(
        &[r, s].concat(),
        recovery_id
    ).map_err(|e| format!("Failed to create signature: {}", e))?;
    
    // Create message hash as secp256k1 message
    let msg = secp256k1::Message::from_slice(&message_hash[..])
        .map_err(|e| format!("Failed to create message: {}", e))?;
    
    // Recover the public key
    let pubkey = secp.recover_ecdsa(&msg, &sig)
        .map_err(|e| format!("Failed to recover public key: {}", e))?;
    
    // Convert public key to Ethereum address format (last 20 bytes of keccak256 hash of public key)
    let pubkey_bytes = pubkey.serialize_uncompressed();
    // Remove the 0x04 prefix for hashing
    let pubkey_for_hash = &pubkey_bytes[1..];
    let mut pubkey_hasher = Keccak256::new();
    pubkey_hasher.update(pubkey_for_hash);
    let pubkey_hash = pubkey_hasher.finalize();
    let recovered_address = format!("0x{}", hex::encode(&pubkey_hash[12..])); // Last 20 bytes
    
    tracing::debug!("[VERIFY-SIG] Recovered address: {}", recovered_address.to_lowercase());
    
    // Get expected witness address
    let witness_address = witnesses_array
        .first()
        .and_then(|w| w.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| "No witness address found".to_string())?;
    
    tracing::debug!("[VERIFY-SIG] Expected witness: {}", witness_address.to_lowercase());
    
    // Compare addresses
    if recovered_address.to_lowercase() == witness_address.to_lowercase() {
        tracing::info!("[VERIFY-SIG] ✓ SDK signature verification PASSED - Recovered address matches witness");
        Ok(())
    } else {
        Err(format!(
            "Signature verification FAILED - Recovered address {} does not match witness {}",
            recovered_address.to_lowercase(),
            witness_address.to_lowercase()
        ))
    }
}

/// Verifies a proof on-chain via Reclaim smart contract
/// 
/// This sends a transaction to the Reclaim contract on Optimism Sepolia to verify the proof.
/// The contract performs cryptographic verification and transaction status indicates result.
/// 
/// This requires:
/// - SEPOLIA_RPC_URL: RPC endpoint (default: https://sepolia.optimism.io)
/// - RECLAIM_ADDRESS: Contract address (default: 0xAe94FB09711e1c6B057853a515483792d8e474d0)
/// - PRIVATE_KEY: Private key for transaction signer (required for on-chain verification)
async fn verify_onchain_sig(
    proof_data: &serde_json::Value,
) -> Result<(), String> {
    use ethers::prelude::*;
    use ethers::abi::Token;
    use ethers::middleware::SignerMiddleware;
    use ethers::types::transaction::eip2718::TypedTransaction;
    use hex;
    
    // Get configuration from environment
    let rpc_url = std::env::var("SEPOLIA_RPC_URL")
        .unwrap_or_else(|_| "https://sepolia.sepolia.io".to_string());
    
    let contract_address_str = std::env::var("RECLAIM_ADDRESS")
        .unwrap_or_else(|_| "0xAe94FB09711e1c6B057853a515483792d8e474d0".to_string());
    
    // Private key is required for on-chain verification to send transaction
    let private_key_str = std::env::var("PRIVATE_KEY")
        .map_err(|_| "PRIVATE_KEY environment variable required for on-chain verification".to_string())?;
    
    tracing::info!("[VERIFY-SIG] Verifying proof on-chain via contract: {}", contract_address_str);
    
    tracing::debug!("[VERIFY-SIG] On-chain proof structure ready for contract call");
    
    // Step 1: Connect to RPC provider
    tracing::debug!("[VERIFY-SIG] Connecting to RPC provider: {}", rpc_url);
    let provider = Provider::<Http>::try_from(&rpc_url)
        .map_err(|e| format!("Failed to connect to RPC provider: {}", e))?;
    
    // Step 2: Get network info
    let network = provider.get_chainid()
        .await
        .map_err(|e| format!("Failed to get chain ID: {}", e))?;
    tracing::debug!("[VERIFY-SIG] Connected to chain ID: {}", network);
    
    // Step 3: Create signer from private key
    let wallet: LocalWallet = private_key_str.parse()
        .map_err(|e| format!("Invalid private key format: {}", e))?;
    let signer = wallet.with_chain_id(network.as_u64());
    let signer_address = signer.address();
    tracing::debug!("[VERIFY-SIG] Signer created for address: {}", signer_address);
    
    // Step 4: Check signer balance for gas
    let balance = provider.get_balance(signer_address, None)
        .await
        .map_err(|e| format!("Failed to check account balance: {}", e))?;
    let balance_eth = balance.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
    tracing::debug!("[VERIFY-SIG] Account balance: {} ETH", balance_eth);
    
    if balance < U256::from(10_000_000_000_000_000u64) { // 0.01 ETH minimum
        return Err(format!("Insufficient balance for gas fees. Current: {} ETH, Required: >= 0.01 ETH", balance_eth));
    }
    
    // Step 5: Parse contract address
    let contract_addr: Address = contract_address_str.parse()
        .map_err(|e| format!("Invalid contract address: {}", e))?;
    tracing::debug!("[VERIFY-SIG] Contract address parsed: {}", contract_addr);
    
    // Step 6: Verify contract exists
    let code = provider.get_code(contract_addr, None)
        .await
        .map_err(|e| format!("Failed to check contract deployment: {}", e))?;
    
    if code.is_empty() {
        return Err(format!("Contract not deployed at address: {}", contract_addr));
    }
    tracing::debug!("[VERIFY-SIG] Contract verified as deployed");
    
    // Create signer client early so we can use it for gas estimation and transaction sending
    let client = SignerMiddleware::new(provider, signer);
    
    // Step 7: Extract onchainProof - it might be at the top level or nested
    // Structure could be:
    // 1. { onchainProof: {...}, proof: {...} }  - from agent-a via attestation
    // 2. { claimInfo: {...}, signedClaim: {...} } - direct onchain proof
    let onchain_proof_value = if let Some(op) = proof_data.get("onchainProof") {
        tracing::debug!("[VERIFY-SIG] Found onchainProof at top level");
        op.clone()
    } else if proof_data.get("claimInfo").is_some() {
        // Proof is already in onchain format
        tracing::debug!("[VERIFY-SIG] Proof is already in onchain format (has claimInfo)");
        proof_data.clone()
    } else {
        return Err("Missing onchainProof or claimInfo in proof_data".to_string());
    };
    
    // DEBUG: Log the structure of onchain_proof_value
    tracing::debug!("[VERIFY-SIG] onchain_proof_value keys: {:?}", onchain_proof_value.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    if let Some(signed_claim) = onchain_proof_value.get("signedClaim") {
        tracing::debug!("[VERIFY-SIG] signedClaim keys: {:?}", signed_claim.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    }
    
    // Extract ClaimInfo fields
    let claim_info = onchain_proof_value.get("claimInfo").ok_or("Missing claimInfo")?;
    let provider = claim_info.get("provider").and_then(|v| v.as_str()).ok_or("Missing provider")?;
    let parameters = claim_info.get("parameters").and_then(|v| v.as_str()).ok_or("Missing parameters")?;
    let context = claim_info.get("context").and_then(|v| v.as_str()).ok_or("Missing context")?;
    
    tracing::debug!("[VERIFY-SIG] ClaimInfo fields:");
    tracing::debug!("[VERIFY-SIG]   provider: {:?}", provider);
    tracing::debug!("[VERIFY-SIG]   parameters: {:?}", parameters);
    tracing::debug!("[VERIFY-SIG]   context: {:?}", context);
    
    // NOTE: According to Reclaim Protocol docs, we should NOT verify that
    // identifier == keccak256(abi.encodePacked(provider + parameters + context))
    // The identifier is generated by Reclaim attestors server-side and cannot be locally computed.
    // The contract verifies witness signatures and accepts the identifier as-is.
    
    // Extract SignedClaim data
    let signed_claim = onchain_proof_value.get("signedClaim").ok_or("Missing signedClaim")?;
    
    // Extract CompleteClaimData fields from SignedClaim.claim (not claimData)
    let claim_data = signed_claim.get("claim").ok_or("Missing claim in signedClaim")?;
    let identifier_str = claim_data.get("identifier").and_then(|v| v.as_str()).ok_or("Missing identifier")?;
    let owner_str = claim_data.get("owner").and_then(|v| v.as_str()).ok_or("Missing owner")?;
    let claim = claim_data.clone();
    
    tracing::info!("[VERIFY-SIG] Identifier from proof: {}", identifier_str);
    
    // Parse identifier from hex to H256
    let identifier_h256: H256 = identifier_str.parse()
        .map_err(|e| format!("Invalid identifier hex: {}", e))?;
    
    // Build ClaimInfo tuple token: (string, string, string)
    let claim_info_token = Token::Tuple(vec![
        Token::String(provider.to_string()),
        Token::String(parameters.to_string()),
        Token::String(context.to_string()),
    ]);
    let owner_addr: Address = owner_str.parse()
        .map_err(|e| format!("Invalid owner address: {}", e))?;
    
    let timestamp = claim.get("timestampS").and_then(|v| v.as_u64()).ok_or("Missing timestampS")? as u32;
    let epoch = claim.get("epoch").and_then(|v| v.as_u64()).ok_or("Missing epoch")? as u32;
    
    // Build CompleteClaimData tuple token: (bytes32, address, uint32, uint32)
    let complete_claim_token = Token::Tuple(vec![
        Token::FixedBytes(identifier_h256.as_bytes().to_vec()),
        Token::Address(owner_addr),
        Token::Uint(U256::from(timestamp)),
        Token::Uint(U256::from(epoch)),
    ]);
    
    // Extract signatures array: bytes[]
    let signatures = signed_claim.get("signatures").and_then(|v| v.as_array()).ok_or("Missing signatures")?;
    let sig_tokens: Result<Vec<Token>, String> = signatures
        .iter()
        .map(|sig| {
            let sig_str = sig.as_str().ok_or_else(|| "Signature is not a string".to_string())?;
            let sig_bytes = hex::decode(sig_str.trim_start_matches("0x"))
                .map_err(|e| format!("Failed to decode signature: {}", e))?;
            Ok(Token::Bytes(sig_bytes))
        })
        .collect();
    
    // Build SignedClaim tuple token: (CompleteClaimData, bytes[])
    let signed_claim_token = Token::Tuple(vec![
        complete_claim_token,
        Token::Array(sig_tokens?),
    ]);
    
    // Build full Proof tuple: (ClaimInfo, SignedClaim)
    let proof_token = Token::Tuple(vec![
        claim_info_token,
        signed_claim_token,
    ]);
    
    // Encode the proof as ABI-compliant tuple
    let encoded_params = ethers::abi::encode(&vec![proof_token]);
    
    tracing::debug!("[VERIFY-SIG] Proof encoded as ABI-compliant tuple structure");
    tracing::debug!("[VERIFY-SIG] Encoded params hex: 0x{}", hex::encode(&encoded_params));
    
    // Compute function selector using the full canonical signature
    // ethers.js computes this from the complete nested struct type definition
    // NOTE: The entire Proof struct is ONE parameter (a single tuple), so wrap in outer parentheses
    let full_signature = "verifyProof(((string,string,string),((bytes32,address,uint32,uint32),bytes[])))";
    let mut selector_hasher = Keccak256::new();
    selector_hasher.update(full_signature.as_bytes());
    let selector_hash = selector_hasher.finalize();
    let function_selector = &selector_hash[0..4]; // First 4 bytes
    
    tracing::info!("[VERIFY-SIG] Function signature: {}", full_signature);
    tracing::info!("[VERIFY-SIG] Function selector: 0x{}", hex::encode(function_selector));
    
    let mut call_data = function_selector.to_vec();
    call_data.extend_from_slice(&encoded_params);
    
    tracing::info!("[VERIFY-SIG] Full call_data hex: 0x{}", hex::encode(&call_data));
    tracing::info!("[VERIFY-SIG] Call_data length: {} bytes", call_data.len());
    
    // DEBUG: Log key fields being sent
    tracing::info!("[VERIFY-SIG] Proof data summary:");
    tracing::info!("[VERIFY-SIG]   Provider: {}", provider);
    tracing::info!("[VERIFY-SIG]   Owner: {}", owner_str);
    tracing::info!("[VERIFY-SIG]   Timestamp: {}", timestamp);
    tracing::info!("[VERIFY-SIG]   Epoch: {}", epoch);
    tracing::info!("[VERIFY-SIG]   Identifier: {}", identifier_str);
    tracing::info!("[VERIFY-SIG]   Signatures count: {}", signatures.len());
    
    // Step 8: Build transaction request
    let tx_request = TransactionRequest::new()
        .to(contract_addr)
        .data(call_data.clone());
    
    // Convert to TypedTransaction for gas estimation
    let typed_tx = TypedTransaction::Legacy(tx_request.clone());
    
    // Estimate gas for the transaction
    let gas_estimate = client.estimate_gas(&typed_tx, None)
        .await
        .map_err(|e| format!("Gas estimation failed: {}", e))?;
    
    tracing::debug!("[VERIFY-SIG] Estimated gas: {}", gas_estimate);
    
    // Step 9: Build and send transaction with estimated gas
    let tx_request_with_gas = tx_request.gas(gas_estimate);
    
    tracing::debug!("[VERIFY-SIG] Sending transaction to contract...");
    let pending_tx = client.send_transaction(tx_request_with_gas, None)
        .await
        .map_err(|e| format!("Failed to send transaction: {}", e))?;
    
    let tx_hash = pending_tx.tx_hash();
    tracing::info!("[VERIFY-SIG] Transaction submitted with hash: {}", tx_hash);
    
    // Step 10: Wait for transaction confirmation
    tracing::debug!("[VERIFY-SIG] Waiting for transaction confirmation...");
    let receipt = pending_tx.confirmations(1)
        .await
        .map_err(|e| format!("Failed to wait for transaction confirmation: {}", e))?
        .ok_or_else(|| "Transaction receipt not found".to_string())?;
    
    tracing::debug!("[VERIFY-SIG] Transaction confirmed in block: {:?}", receipt.block_number);
    if let Some(gas_used) = receipt.gas_used {
        tracing::debug!("[VERIFY-SIG] Gas used: {}", gas_used);
    }
    
    // Step 11: Check transaction status
    let verified = receipt.status
        .map(|status| status.as_u64() == 1)
        .unwrap_or(false);
    
    tracing::info!("[VERIFY-SIG] Transaction receipt status: {:?}", receipt.status);
    if let Some(gas_used) = receipt.gas_used {
        tracing::debug!("[VERIFY-SIG] Gas used: {}", gas_used);
    }
    
    if verified {
        tracing::info!("[VERIFY-SIG] ✓ On-chain verification PASSED - Transaction status: success");
        Ok(())
    } else {
        Err("On-chain verification FAILED - Transaction reverted or failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identifier_hash_computation() {
        // These are the EXACT string values from the working proof
        // They must match byte-for-byte with what the attestation service provides
        let provider = "http";
        let parameters = r#"{"body":"","headers":{"User-Agent":"reclaim/0.0.1","accept":"application/json"},"method":"GET","responseMatches":[{"type":"regex","value":"\"origin\":\\s*\"(?<origin>[^\"]+)\""}],"responseRedactions":[],"url":"https://httpbin.org/get"}"#;
        let context = r#"{"extractedParameters":{"origin":"3.110.82.84"},"providerHash":"0x245a11f715ca085fabe2986526a51e43f286650f992dde2d036daf2f16fc1370"}"#;
        
        // Compute identifier hash the way Solidity does
        let mut hasher = Keccak256::new();
        hasher.update(provider.as_bytes());
        hasher.update(parameters.as_bytes());
        hasher.update(context.as_bytes());
        let computed = hasher.finalize();
        
        let computed_hex = format!("0x{}", hex::encode(&computed[..]));
        let expected_hex = "0x2bd1cc71a31100fe3e6137cd6d19cde93d371047827bb0f13f66572e191cd82e";
        
        eprintln!("Computed: {}", computed_hex);
        eprintln!("Expected: {}", expected_hex);
        eprintln!("Parameters length: {}", parameters.len());
        eprintln!("Parameters: {}", parameters);
        
        // Note: If this fails, it means the JSON in proof-structure.json wasn't the true source
        assert_eq!(
            computed_hex.to_lowercase(),
            expected_hex.to_lowercase(),
            "Identifier hash mismatch! Check that the parameter strings match exactly (no escaping differences)."
        );
    }
}

