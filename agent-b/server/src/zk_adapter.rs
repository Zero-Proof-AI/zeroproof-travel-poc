/// ZK Input Adapter for Agent B
/// 
/// This module provides utilities to convert HTTP request formats
/// to zkVM input formats. This keeps Agent B's internal zkVM structure
/// private while allowing external agents to interact via simple JSON.

use serde_json::Value;
use pricing_core::{pricing, booking, RpcCall};

/// Convert generic JSON input to Agent B's internal RpcCall format
/// This allows Agent A to send simple JSON without knowing RpcCall structure
pub fn json_to_rpc_call(endpoint: &str, input: &Value) -> Result<RpcCall, String> {
    match endpoint {
        "price" => {
            let req: pricing::Request = serde_json::from_value(input.clone())
                .map_err(|e| format!("Invalid pricing input: {}", e))?;
            Ok(RpcCall::GetPrice(req))
        }
        "book" => {
            let req: booking::Request = serde_json::from_value(input.clone())
                .map_err(|e| format!("Invalid booking input: {}", e))?;
            Ok(RpcCall::BookFlight(req))
        }
        _ => Err(format!("Unknown endpoint: {}", endpoint))
    }
}

/// Helper to serialize RpcCall to bincode bytes for zkVM
pub fn rpc_call_to_bytes(call: &RpcCall) -> Vec<u8> {
    bincode::serialize(call).expect("Failed to serialize RpcCall")
}
