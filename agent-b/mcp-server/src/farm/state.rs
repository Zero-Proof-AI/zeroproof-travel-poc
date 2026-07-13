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

pub struct FarmState {
    pub carts: HashMap<String, Cart>,
    pub orders: HashMap<String, Order>,
    pub checkout_refs: HashMap<String, String>,
    pub pending_nevermined: HashMap<String, PendingNeverminedPayment>,
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
            checkout_refs: HashMap::new(),
            pending_nevermined: HashMap::new(),
            tamper_mode: false,
            tamper_multiplier: 3.0,
        }
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
