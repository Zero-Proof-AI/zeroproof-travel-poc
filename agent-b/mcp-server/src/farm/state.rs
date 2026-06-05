use farm_core::types::{Cart, Order};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::db::SharedMerchantDb;

#[derive(Clone, Debug)]
pub struct PendingNeverminedPayment {
    pub merchant_url: String,
    pub amount_cents: u64,
    pub description: String,
}

/// Records that the merchant independently verified a ZPI proof for an
/// `external_id` (Delta #5). ZPI-ZKPay reads this over localhost before it
/// mints an x402 token, so a credential is never minted for an intent the
/// merchant has not validated.
#[derive(Clone, Debug)]
pub struct VerifiedIntent {
    pub amount_cents: u64,
    pub currency: String,
    pub merchant_url: String,
    /// Canonical proof UUID stored by zk-attestation-service.
    pub proof_id: String,
    pub verified_at: std::time::SystemTime,
}

/// How long a recorded merchant verification stays valid — must outlive the
/// pending-intent window so mint + settle can still find it.
pub const VERIFIED_INTENT_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub struct FarmState {
    pub carts: HashMap<String, Cart>,
    pub orders: HashMap<String, Order>,
    pub pending_nevermined: HashMap<String, PendingNeverminedPayment>,
    /// external_id → merchant-side proof verification (Delta #5).
    pub verified_intents: HashMap<String, VerifiedIntent>,
    /// When true, checkout inflates the total by a multiplier for unhappy-path testing.
    pub tamper_mode: bool,
    /// Multiplier applied to the order total when tamper_mode is on (default: 3.0).
    pub tamper_multiplier: f64,
}

impl FarmState {
    pub fn new() -> Self {
        Self {
            carts: HashMap::new(),
            orders: HashMap::new(),
            pending_nevermined: HashMap::new(),
            verified_intents: HashMap::new(),
            tamper_mode: false,
            tamper_multiplier: 3.0,
        }
    }

    /// Drop expired merchant-verification records.
    pub fn prune_verified_intents(&mut self) {
        let now = std::time::SystemTime::now();
        self.verified_intents.retain(|_, v| {
            now.duration_since(v.verified_at)
                .map(|age| age < VERIFIED_INTENT_TTL)
                .unwrap_or(true)
        });
    }
}

pub type SharedFarmState = Arc<RwLock<FarmState>>;

pub fn new_shared_state() -> SharedFarmState {
    Arc::new(RwLock::new(FarmState::new()))
}

/// Combined application state — derive sub-states via `FromRef`.
#[derive(Clone)]
pub struct AppState {
    pub farm: SharedFarmState,
    pub merchant_db: SharedMerchantDb,
}

impl axum::extract::FromRef<AppState> for SharedFarmState {
    fn from_ref(state: &AppState) -> Self {
        state.farm.clone()
    }
}

impl axum::extract::FromRef<AppState> for SharedMerchantDb {
    fn from_ref(state: &AppState) -> Self {
        state.merchant_db.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prune_verified_intents_drops_stale_keeps_fresh() {
        let now = std::time::SystemTime::now();
        let stale_at = now
            .checked_sub(VERIFIED_INTENT_TTL + std::time::Duration::from_secs(1))
            .expect("stale timestamp should be representable");

        let mut state = FarmState::new();
        state.verified_intents.insert(
            "fresh-ext".into(),
            VerifiedIntent {
                amount_cents: 100,
                currency: "USD".into(),
                merchant_url: "https://fresh.example".into(),
                proof_id: "attester-fresh".into(),
                verified_at: now,
            },
        );
        state.verified_intents.insert(
            "stale-ext".into(),
            VerifiedIntent {
                amount_cents: 200,
                currency: "USD".into(),
                merchant_url: "https://stale.example".into(),
                proof_id: "attester-stale".into(),
                verified_at: stale_at,
            },
        );

        state.prune_verified_intents();

        assert!(state.verified_intents.contains_key("fresh-ext"));
        assert!(!state.verified_intents.contains_key("stale-ext"));
        assert_eq!(state.verified_intents.len(), 1);
    }
}
