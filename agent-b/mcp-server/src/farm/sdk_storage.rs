use super::db::SharedMerchantDb;
use zk_agentic_sdk::{MerchantSdkError, MerchantSdkStorage};

/// Storage adapter that bridges the SDK's MerchantSdkStorage trait with agent-b's SQLite DB.
pub struct AgentBSdkStorage {
    db: SharedMerchantDb,
}

impl AgentBSdkStorage {
    pub fn new(db: SharedMerchantDb) -> Self {
        Self { db }
    }
}

impl MerchantSdkStorage for AgentBSdkStorage {
    fn get_s1k(&self, _merchant_id: &str) -> Result<Option<String>, MerchantSdkError> {
        // Agent-B uses a single merchant per instance, so we ignore the merchant_id parameter
        // and retrieve the stored s1k from the local DB.
        Ok(self.db.get_s1k())
    }

    fn set_s1k(
        &self,
        _merchant_id: &str,
        s1k: &str,
        expires_at: Option<String>,
    ) -> Result<(), MerchantSdkError> {
        // Store the s1k with optional expiry metadata
        self.db
            .save_s1k(s1k, expires_at.as_deref())
            .map_err(|e| MerchantSdkError::Api {
                status: 500,
                body: format!("Failed to store s1k: {}", e),
            })
    }
}
