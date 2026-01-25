use sha3::{Keccak256, Digest};
use hex;

fn main() {
    // Working proof values from the httpbin example
    let provider = "http";
    let parameters = r#"{"body":"","headers":{"User-Agent":"reclaim/0.0.1","accept":"application/json"},"method":"GET","responseMatches":[{"type":"regex","value":"\"origin\":\\s*\"(?<origin>[^\"]+)\""}],"responseRedactions":[],"url":"https://httpbin.org/get"}"#;
    let context = r#"{"extractedParameters":{"origin":"3.110.82.84"},"providerHash":"0x245a11f715ca085fabe2986526a51e43f286650f992dde2d036daf2f16fc1370"}"#;
    
    // Compute identifier hash the way Solidity does: keccak256(abi.encodePacked(...))
    let mut hasher = Keccak256::new();
    hasher.update(provider.as_bytes());
    hasher.update(parameters.as_bytes());
    hasher.update(context.as_bytes());
    let computed = hasher.finalize();
    
    let computed_hex = format!("0x{}", hex::encode(&computed[..]));
    let expected_hex = "0x2bd1cc71a31100fe3e6137cd6d19cde93d371047827bb0f13f66572e191cd82e";
    
    println!("=== Testing Rust Identifier Hash ===");
    println!("Computed: {}", computed_hex);
    println!("Expected: {}", expected_hex);
    println!("Match: {}", computed_hex.to_lowercase() == expected_hex.to_lowercase());
    
    if computed_hex.to_lowercase() == expected_hex.to_lowercase() {
        println!("\n✓ SUCCESS: Rust hash computation is correct!");
        println!("This means our ABI encoding will also be correct.");
    } else {
        println!("\n✗ FAILED: Hash mismatch!");
        println!("This would indicate an issue with our hash computation.");
    }
}
