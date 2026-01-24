use secp256k1::{Secp256k1, ecdsa::RecoveryId};
use sha3::{Keccak256, Digest};

/// Verifies a proof's cryptographic signature using secp256k1
/// 
/// This implements the Reclaim protocol's signature verification:
/// 1. Extracts claim data, signatures, and witnesses from proof_data
/// 2. Reconstructs the signed message in format: identifier\nowner\ntimestamp\nepoch
/// 3. Adds Ethereum prefix: "\x19Ethereum Signed Message:\n<length>"
/// 4. Hashes with keccak256
/// 5. Recovers the signer from the signature via secp256k1
/// 6. Compares recovered signer with expected witness
pub fn verify_secp256k1_sig(
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
    
    tracing::debug!("[VERIFY-PAYMENT] Message to verify: {:?}", message);
    tracing::debug!("[VERIFY-PAYMENT] Signature: {}", signature_str);
    
    // Add Ethereum message prefix and hash
    let prefixed_message = format!(
        "\x19Ethereum Signed Message:\n{}{}",
        message.len(),
        message
    );
    
    let mut hasher = Keccak256::new();
    hasher.update(prefixed_message.as_bytes());
    let message_hash = hasher.finalize();
    tracing::debug!("[VERIFY-PAYMENT] Message hash: 0x{}", hex::encode(&message_hash[..]));
    
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
    
    tracing::debug!("[VERIFY-PAYMENT] Recovered address: {}", recovered_address.to_lowercase());
    
    // Get expected witness address
    let witness_address = witnesses_array
        .first()
        .and_then(|w| w.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| "No witness address found".to_string())?;
    
    tracing::debug!("[VERIFY-PAYMENT] Expected witness: {}", witness_address.to_lowercase());
    
    // Compare addresses
    if recovered_address.to_lowercase() == witness_address.to_lowercase() {
        tracing::info!("[VERIFY-PAYMENT] ✓ Signature verification PASSED - Recovered address matches witness");
        Ok(())
    } else {
        Err(format!(
            "Signature verification FAILED - Recovered address {} does not match witness {}",
            recovered_address.to_lowercase(),
            witness_address.to_lowercase()
        ))
    }
}
