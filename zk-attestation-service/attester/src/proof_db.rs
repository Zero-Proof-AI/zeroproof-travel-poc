/// Proof storage and retrieval module
/// Stores cryptographic proofs in an in-memory database
/// In production, this should use a persistent database like PostgreSQL

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

/// Redaction metadata (same as agent-a)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionMetadata {
    pub redacted_field_count: usize,
    pub redacted_paths: Vec<String>,
    pub was_redacted: bool,
}

impl Default for RedactionMetadata {
    fn default() -> Self {
        Self {
            redacted_field_count: 0,
            redacted_paths: Vec::new(),
            was_redacted: false,
        }
    }
}

/// Stored proof record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProof {
    pub proof_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub timestamp: u64,
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub proof: serde_json::Value,
    pub verified: bool,
    pub onchain_compatible: bool,
    pub submitted_by: Option<String>, // Which agent submitted this proof (agent-a, agent-b, payment-agent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u32>, // Order in the workflow (1, 2, 3, ...)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_proof_id: Option<String>, // Reference to parent/related proof (for dependency tracking)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_stage: Option<String>, // e.g., "pricing", "payment_enrollment", "payment", "booking"
    
    /// Display version of response with sensitive fields redacted (for UI)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_response: Option<serde_json::Value>,
    
    /// Metadata about which fields were redacted and why
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction_metadata: Option<RedactionMetadata>,
}

/// In-memory proof database (sync version for Axum handlers)
pub struct ProofDatabase {
    proofs: Arc<RwLock<HashMap<String, Vec<StoredProof>>>>, // session_id -> proofs
}

impl ProofDatabase {
    pub fn new() -> Self {
        Self {
            proofs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a proof in the database
    pub fn store_proof(&self, proof: StoredProof) -> Result<String, String> {
        let mut db = self.proofs.write().map_err(|e| format!("Lock error: {}", e))?;
        
        let proof_id = proof.proof_id.clone();
        let session_id = proof.session_id.clone();
        
        db.entry(session_id)
            .or_insert_with(Vec::new)
            .push(proof);
        
        Ok(proof_id)
    }

    /// Retrieve all proofs for a session, sorted by timestamp
    pub fn get_proofs(&self, session_id: &str) -> Result<Vec<StoredProof>, String> {
        let db = self.proofs.read().map_err(|e| format!("Lock error: {}", e))?;
        
        let mut proofs = db
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        
        // Sort by timestamp to maintain chronological order
        proofs.sort_by_key(|p| p.timestamp);
        
        Ok(proofs)
    }

    /// Retrieve a specific proof by ID
    pub fn get_proof(&self, proof_id: &str) -> Result<Option<StoredProof>, String> {
        let db = self.proofs.read().map_err(|e| format!("Lock error: {}", e))?;
        
        // Search through all sessions to find the proof
        for proofs in db.values() {
            for proof in proofs {
                if proof.proof_id == proof_id {
                    return Ok(Some(proof.clone()));
                }
            }
        }
        
        Ok(None)
    }

    /// Get proof count for a session
    pub fn get_proof_count(&self, session_id: &str) -> Result<usize, String> {
        let db = self.proofs.read().map_err(|e| format!("Lock error: {}", e))?;
        Ok(db.get(session_id).map(|p| p.len()).unwrap_or(0))
    }

    /// Clear proofs for a session
    pub fn clear_proofs(&self, session_id: &str) -> Result<usize, String> {
        let mut db = self.proofs.write().map_err(|e| format!("Lock error: {}", e))?;
        Ok(db.remove(session_id).map(|p| p.len()).unwrap_or(0))
    }
}

// HTTP Request/Response types
#[derive(Debug, Serialize, Deserialize)]
pub struct ProofSubmissionRequest {
    pub session_id: String,
    pub tool_name: String,
    pub timestamp: u64,
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub proof: serde_json::Value,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub onchain_compatible: bool,
    pub submitted_by: Option<String>,
    pub sequence: Option<u32>,
    pub related_proof_id: Option<String>,
    pub workflow_stage: Option<String>,
    pub display_response: Option<serde_json::Value>,
    pub redaction_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ProofSubmissionResponse {
    pub success: bool,
    pub proof_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SingleProofResponse {
    pub success: bool,
    pub data: Option<SingleProofData>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SingleProofData {
    pub proof: StoredProof,
    pub verification_info: VerificationInfo,
}

#[derive(Debug, Serialize)]
pub struct VerificationInfo {
    pub protocol: String,
    pub issuer: String,
    pub timestamp_verified: bool,
    pub signature_algorithm: String,
    pub can_verify_onchain: bool,
}

#[derive(Debug, Serialize)]
pub struct ProofsResponse {
    pub success: bool,
    pub session_id: String,
    pub count: usize,
    pub proofs: Vec<StoredProof>,
    pub verification_metadata: VerificationMetadata,
}

#[derive(Debug, Serialize)]
pub struct VerificationMetadata {
    pub protocol: String,
    pub issuer: String,
    pub verification_service: String,
}

#[derive(Debug, Serialize)]
pub struct ProofCountResponse {
    pub success: bool,
    pub session_id: String,
    pub count: usize,
}

impl Clone for ProofDatabase {
    fn clone(&self) -> Self {
        Self {
            proofs: self.proofs.clone(),
        }
    }
}

impl Default for ProofDatabase {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_retrieve_proof() {
        let db = ProofDatabase::new();
        
        let proof = StoredProof {
            proof_id: "proof_1".to_string(),
            session_id: "session_1".to_string(),
            tool_name: "get-ticket-price".to_string(),
            timestamp: 1234567890,
            request: serde_json::json!({ "from": "NYC" }),
            response: serde_json::json!({ "price": 450 }),
            proof: serde_json::json!({ "verified": true }),
            verified: true,
            onchain_compatible: true,
            submitted_by: Some("agent-a".to_string()),
            sequence: Some(1),
            related_proof_id: None,
            workflow_stage: Some("pricing".to_string()),
            display_response: None,
            redaction_metadata: None,
        };
        
        db.store_proof(proof.clone()).unwrap();
        
        let proofs = db.get_proofs("session_1").unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].proof_id, "proof_1");
    }
}
