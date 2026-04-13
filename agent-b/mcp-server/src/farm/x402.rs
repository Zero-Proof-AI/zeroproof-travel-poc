use farm_core::types::Order;
use serde::{Deserialize, Serialize};

// ── x402 wire types ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequired {
    #[serde(rename = "x402Version")]
    pub x402_version: u32,
    pub resource: PaymentResource,
    pub accepts: Vec<PaymentAccept>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResource {
    pub url: String,
    pub description: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentAccept {
    pub scheme: String,
    pub network: String,
    pub amount: String,
    pub asset: String,
    #[serde(rename = "payTo")]
    pub pay_to: String,
    #[serde(rename = "maxTimeoutSeconds")]
    pub max_timeout_seconds: u32,
    pub extra: PaymentExtra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentExtra {
    #[serde(rename = "assetTransferMethod")]
    pub asset_transfer_method: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SettlementRequest {
    pub payment_payload: String,
    pub expected_pay_to: String,
    pub expected_amount: String,
    pub expected_asset: String,
    pub expected_network: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SettlementResponse {
    pub success: bool,
    pub tx_hash: Option<String>,
    pub network: Option<String>,
    pub error: Option<String>,
}

// ── Config ───────────────────────────────────────────────────────

/// Per-chain payment config used in x402 `accepts` entries.
#[derive(Debug, Clone)]
pub struct ChainPaymentConfig {
    pub network: String,     // e.g. "eip155:84532"
    pub usdc_contract: String,
}

pub struct X402Config {
    pub merchant_wallet: String,
    pub zpi_zkpay_url: String,
    /// Supported chains, in order of preference.
    pub chains: Vec<ChainPaymentConfig>,
    pub server_base_url: String,
}

/// Default chain configs (same USDC addresses used in enrollment.rs).
fn default_chains() -> Vec<ChainPaymentConfig> {
    vec![
        ChainPaymentConfig {
            network: "eip155:11155111".into(),
            usdc_contract: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238".into(),
        },
        ChainPaymentConfig {
            network: "eip155:84532".into(),
            usdc_contract: "0x036CbD53842c5426634e7929541eC2318f3dCF7e".into(),
        },
        ChainPaymentConfig {
            network: "eip155:300".into(),
            usdc_contract: "0xAe045DE5638162fa134807Cb558E15A3F5A7F853".into(),
        },
    ]
}

impl X402Config {
    pub fn from_env() -> Self {
        // PAYMENT_NETWORKS overrides defaults: comma-separated "eip155:84532,eip155:8453"
        let chains = match std::env::var("PAYMENT_NETWORKS") {
            Ok(val) => val
                .split(',')
                .filter_map(|net| {
                    let net = net.trim();
                    if net.is_empty() {
                        return None;
                    }
                    // Derive chain_id from "eip155:<id>"
                    let chain_id: u64 = net
                        .strip_prefix("eip155:")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let usdc = super::enrollment::usdc_address(chain_id).to_string();
                    Some(ChainPaymentConfig {
                        network: net.to_string(),
                        usdc_contract: usdc,
                    })
                })
                .collect(),
            Err(_) => {
                // Legacy single-network fallback
                if let Ok(net) = std::env::var("PAYMENT_NETWORK") {
                    let chain_id: u64 = net
                        .strip_prefix("eip155:")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(84532);
                    let usdc = std::env::var("USDC_CONTRACT").unwrap_or_else(|_| {
                        super::enrollment::usdc_address(chain_id).to_string()
                    });
                    vec![ChainPaymentConfig {
                        network: net,
                        usdc_contract: usdc,
                    }]
                } else {
                    default_chains()
                }
            }
        };

        Self {
            merchant_wallet: std::env::var("MERCHANT_WALLET_ADDRESS")
                .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".into()),
            zpi_zkpay_url: std::env::var("ZPI_ZKPAY_URL")
                .unwrap_or_else(|_| "http://localhost:3002".into()),
            chains,
            server_base_url: std::env::var("SERVER_BASE_URL")
                .unwrap_or_else(|_| {
                    let port = std::env::var("PORT").unwrap_or_else(|_| "8001".into());
                    format!("http://localhost:{}", port)
                }),
        }
    }
}

// ── Builder ──────────────────────────────────────────────────────

/// Build a PaymentRequired challenge for a given order.
/// Emits one `accepts` entry per supported chain so the payer can choose.
pub fn build_payment_required(order: &Order, config: &X402Config) -> PaymentRequired {
    // Convert cents to USDC atomic units (6 decimals).
    // cents × 10^4 = atomic units
    let amount_atomic = order.total_cents * 10_000;

    let accepts: Vec<PaymentAccept> = config
        .chains
        .iter()
        .map(|chain| PaymentAccept {
            scheme: "exact".into(),
            network: chain.network.clone(),
            amount: amount_atomic.to_string(),
            asset: chain.usdc_contract.clone(),
            pay_to: config.merchant_wallet.clone(),
            max_timeout_seconds: 60,
            extra: PaymentExtra {
                asset_transfer_method: "eip3009".into(),
                name: "USDC".into(),
                version: "2".into(),
            },
        })
        .collect();

    PaymentRequired {
        x402_version: 2,
        resource: PaymentResource {
            url: format!("{}/farm/checkout/{}", config.server_base_url, order.order_id),
            description: format!(
                "Farm order: {} item(s), ${:.2}",
                order.items.len(),
                order.total_cents as f64 / 100.0
            ),
            mime_type: "application/json".into(),
        },
        accepts,
    }
}

/// Forward the X-PAYMENT header to zpi-zkpay for verification + on-chain settlement.
/// `network` selects which chain the payment was made on (e.g. "eip155:84532").
/// If None, uses the first configured chain.
pub async fn settle_payment(
    payment_payload_b64: &str,
    order: &Order,
    config: &X402Config,
    network: Option<&str>,
) -> Result<SettlementResponse, String> {
    let amount_atomic = order.total_cents * 10_000;

    // Find the matching chain config
    let chain = match network {
        Some(net) => config
            .chains
            .iter()
            .find(|c| c.network == net)
            .unwrap_or(&config.chains[0]),
        None => &config.chains[0],
    };

    let req = SettlementRequest {
        payment_payload: payment_payload_b64.to_string(),
        expected_pay_to: config.merchant_wallet.clone(),
        expected_amount: amount_atomic.to_string(),
        expected_asset: chain.usdc_contract.clone(),
        expected_network: chain.network.clone(),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/x402/settle", config.zpi_zkpay_url))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Failed to reach zpi-zkpay: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("zpi-zkpay returned {}: {}", status, body));
    }

    resp.json::<SettlementResponse>()
        .await
        .map_err(|e| format!("Failed to parse settlement response: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use farm_core::types::{CartItem, OrderStatus, PaymentMethod};

    fn test_order() -> Order {
        Order {
            order_id: "ord-123".into(),
            session_id: "sess-abc".into(),
            items: vec![CartItem {
                product_id: "farm-eggs-dozen".into(),
                quantity: 2,
                unit_price_cents: 599,
            }],
            total_cents: 1198,
            status: OrderStatus::PendingPayment,
            payment_method: PaymentMethod::X402Crypto,
        }
    }

    fn test_config() -> X402Config {
        X402Config {
            merchant_wallet: "0xTestMerchant".into(),
            zpi_zkpay_url: "http://localhost:3002".into(),
            chains: vec![ChainPaymentConfig {
                network: "eip155:11155111".into(),
                usdc_contract: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238".into(),
            }],
            server_base_url: "http://localhost:8001".into(),
        }
    }

    #[test]
    fn test_build_payment_required_version() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert_eq!(pr.x402_version, 2);
    }

    #[test]
    fn test_build_payment_required_amount() {
        let pr = build_payment_required(&test_order(), &test_config());
        // 1198 cents * 10000 = 11980000 atomic units
        assert_eq!(pr.accepts[0].amount, "11980000");
    }

    #[test]
    fn test_build_payment_required_pay_to() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert_eq!(pr.accepts[0].pay_to, "0xTestMerchant");
    }

    #[test]
    fn test_build_payment_required_resource_url() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert_eq!(pr.resource.url, "http://localhost:8001/farm/checkout/ord-123");
    }

    #[test]
    fn test_build_payment_required_network() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert_eq!(pr.accepts[0].network, "eip155:11155111");
        assert_eq!(pr.accepts[0].scheme, "exact");
    }

    #[test]
    fn test_build_payment_required_erc3009() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert_eq!(pr.accepts[0].extra.asset_transfer_method, "eip3009");
        assert_eq!(pr.accepts[0].extra.name, "USDC");
        assert_eq!(pr.accepts[0].extra.version, "2");
    }

    #[test]
    fn test_build_payment_required_description() {
        let pr = build_payment_required(&test_order(), &test_config());
        assert!(pr.resource.description.contains("$11.98"));
        assert!(pr.resource.description.contains("1 item"));
    }

    #[test]
    fn test_build_payment_required_large_order() {
        let mut order = test_order();
        order.total_cents = 99_99; // $99.99
        let pr = build_payment_required(&order, &test_config());
        assert_eq!(pr.accepts[0].amount, "99990000");
    }
}
