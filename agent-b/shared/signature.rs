use secp256k1::{Secp256k1, ecdsa::RecoveryId};
use sha3::{Keccak256, Digest};

/// Verifies a proof's cryptographic signature
/// 
/// Dispatches to either local SDK verification or on-chain contract verification
/// based on the `onchain` parameter. These are mutually exclusive approaches.
pub async fn verify_secp256k1_sig(
    proof_data: &serde_json::Value,
    onchain: bool,
) -> Result<(), String> {
    if onchain {
        verify_onchain_sig(proof_data).await
    } else {
        verify_sdk_sig(proof_data).await
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
async fn verify_sdk_sig(
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
/// - RECLAIM_RPC_URL: RPC endpoint (default: https://sepolia.optimism.io)
/// - RECLAIM_CONTRACT_ADDRESS: Contract address (default: 0xAe94FB09711e1c6B057853a515483792d8e474d0)
/// - RECLAIM_PRIVATE_KEY: Private key for transaction signer (required for on-chain verification)
async fn verify_onchain_sig(
    proof_data: &serde_json::Value,
) -> Result<(), String> {
    use ethers::prelude::*;
    use ethers::abi::Token;
    use ethers::middleware::SignerMiddleware;
    use ethers::types::transaction::eip2718::TypedTransaction;
    
    // Get configuration from environment
    let rpc_url = std::env::var("RECLAIM_RPC_URL")
        .unwrap_or_else(|_| "https://sepolia.optimism.io".to_string());
    let contract_address_str = std::env::var("RECLAIM_CONTRACT_ADDRESS")
        .unwrap_or_else(|_| "0xAe94FB09711e1c6B057853a515483792d8e474d0".to_string());
    
    // Private key is required for on-chain verification to send transaction
    let private_key_str = std::env::var("RECLAIM_PRIVATE_KEY")
        .map_err(|_| "RECLAIM_PRIVATE_KEY environment variable required for on-chain verification".to_string())?;
    
    tracing::info!("[VERIFY-SIG] Verifying proof on-chain via contract: {}", contract_address_str);
    tracing::debug!("[VERIFY-SIG] RPC URL: {}", rpc_url);
    
    // Extract onchainProof from proof_data for smart contract verification
    let onchain_proof = proof_data
        .get("onchainProof")
        .ok_or_else(|| "Proof missing onchainProof for on-chain verification".to_string())?;
    
    // Verify the proof has required fields for on-chain verification
    // Required: claimInfo (ClaimInfo struct) and signedClaim (SignedClaim struct)
    onchain_proof
        .get("claimInfo")
        .ok_or_else(|| "Proof missing claimInfo for on-chain verification".to_string())?;
    
    onchain_proof
        .get("signedClaim")
        .ok_or_else(|| "Proof missing signedClaim for on-chain verification".to_string())?;
    
    tracing::debug!("[VERIFY-SIG] On-chain proof structure validated");
    
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
    
    // Step 7: Extract and validate proof structure for contract call
    // The contract expects a structured Proof with claimInfo and signedClaim
    // Extract onchainProof which should have the correct structure:
    // {
    //   claimInfo: { provider, parameters, context },
    //   signedClaim: {
    //     claim: { identifier (bytes32), owner (address), timestampS (uint32), epoch (uint32) },
    //     signatures: bytes[]
    //   }
    // }
    let claim_info = onchain_proof
        .get("claimInfo")
        .ok_or_else(|| "Missing claimInfo in proof".to_string())?;
    
    let signed_claim = onchain_proof
        .get("signedClaim")
        .ok_or_else(|| "Missing signedClaim in proof".to_string())?;
    
    // Validate claimInfo structure
    let provider = claim_info
        .get("provider")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing or invalid provider in claimInfo".to_string())?;
    
    // Validate signedClaim.claim structure
    let claim = signed_claim
        .get("claim")
        .ok_or_else(|| "Missing claim in signedClaim".to_string())?;
    
    tracing::debug!("[VERIFY-SIG] Proof structure validated: provider={}, claim has identifier/owner/timestamp/epoch, signatures count=", 
        provider);
    
    // Encode the proof structure for contract call
    // The proof_json will be serialized and encoded as function parameters
    let proof_json = serde_json::to_string(onchain_proof)
        .map_err(|e| format!("Failed to serialize proof: {}", e))?;
    
    // Encode function selector (verifyProof) + parameters
    let tokens = vec![Token::String(proof_json)];
    let encoded_params = ethers::abi::encode(&tokens);
    
    // Function selector for verifyProof(Proof) - compute keccak256 hash of function signature
    let mut selector_hasher = Keccak256::new();
    selector_hasher.update(b"verifyProof(Proof)");
    let selector_hash = selector_hasher.finalize();
    let function_selector = &selector_hash[0..4]; // First 4 bytes
    
    let mut call_data = function_selector.to_vec();
    call_data.extend_from_slice(&encoded_params);
    
    tracing::debug!("[VERIFY-SIG] Function call encoded, call_data length: {} bytes", call_data.len());
    
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
    
    if verified {
        tracing::info!("[VERIFY-SIG] ✓ On-chain verification PASSED - Transaction status: success");
        Ok(())
    } else {
        Err("On-chain verification FAILED - Transaction reverted or failed".to_string())
    }
}
