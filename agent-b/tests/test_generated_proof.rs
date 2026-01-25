use serde_json::Value;
use std::fs;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("\n========================================");
    println!("🧪 Testing Agent B with Generated Proof");
    println!("========================================\n");

    // Load the proof from file
    let proof_json = match fs::read_to_string("/tmp/agent_b_test_proof.json") {
        Ok(content) => content,
        Err(e) => {
            eprintln!("❌ Failed to read proof file: {}", e);
            return;
        }
    };

    let proof_data: Value = match serde_json::from_str(&proof_json) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("❌ Failed to parse proof JSON: {}", e);
            return;
        }
    };

    println!("✅ Proof loaded from /tmp/agent_b_test_proof.json");
    println!("📋 Testing on-chain verification...\n");

    // Call the verification function
    // Note: This imports the shared module from agent-b
    match shared::signature::verify_secp256k1_sig(&proof_data, true, false).await {
        Ok(()) => {
            println!("\n✅ VERIFICATION PASSED!");
            println!("========================================");
            println!("🎉 Agent B successfully verified the proof!");
            println!("========================================\n");
        }
        Err(e) => {
            println!("\n❌ VERIFICATION FAILED!");
            println!("========================================");
            println!("Error: {}", e);
            println!("========================================\n");
        }
    }
}
