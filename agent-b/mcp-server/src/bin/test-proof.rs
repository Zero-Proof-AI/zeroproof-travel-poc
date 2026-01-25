use std::fs;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();

    println!("\n========================================");
    println!("🧪 Testing Agent B with Generated Proof");
    println!("========================================\n");

    // Load the SAME proof file that JavaScript just tested
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
    
    // DEBUG: Show proof structure
    println!("\n📋 Proof Structure Received:");
    if let Some(onchain_proof) = proof_data.get("onchainProof") {
        println!("  ✅ Found 'onchainProof' wrapper");
        if let Some(claim_info) = onchain_proof.get("claimInfo") {
            println!("    ✅ claimInfo exists");
            if let Some(provider) = claim_info.get("provider") {
                println!("      provider: {:?}", provider);
            }
        }
        if let Some(signed_claim) = onchain_proof.get("signedClaim") {
            println!("    ✅ signedClaim exists");
            if let Some(claim) = signed_claim.get("claim") {
                println!("      ✅ claim exists");
                if let Some(identifier) = claim.get("identifier") {
                    println!("        identifier: {:?}", identifier);
                }
                if let Some(owner) = claim.get("owner") {
                    println!("        owner: {:?}", owner);
                }
            }
            if let Some(signatures) = signed_claim.get("signatures") {
                if let Some(arr) = signatures.as_array() {
                    println!("      signatures: {} items", arr.len());
                }
            }
        }
    } else {
        println!("  ❌ No 'onchainProof' wrapper - proof is direct format");
        if let Some(claim_info) = proof_data.get("claimInfo") {
            println!("    ✅ claimInfo exists (direct)");
        }
    }
    
    println!("\n📋 Starting on-chain verification...\n");

    // Call the verification function from shared module
    match shared::signature::verify_secp256k1_sig(&proof_data, true, false).await {
        Ok(()) => {
            println!("\n✅ VERIFICATION PASSED!");
            println!("========================================");
            println!("🎉 Agent B successfully verified the proof!");
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

/// Transform raw proof format to onchain format
/// Raw format: { claimData: {...}, identifier, signatures, witnesses, ... }
/// Onchain format: { onchainProof: { claimInfo: {...}, signedClaim: { claim: {...}, signatures: [...] } } }
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
