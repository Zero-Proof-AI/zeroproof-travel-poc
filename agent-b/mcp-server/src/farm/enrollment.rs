use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client for zpi-zkpay merchant enrollment endpoints.
pub struct ZkpayClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct SendOtpRequest {
    email: String,
}

#[derive(Deserialize)]
pub struct SendOtpResponse {
    pub success: bool,
    pub message: String,
    /// Only populated in dev mode
    pub code: Option<String>,
}

#[derive(Serialize)]
struct MerchantEnrollRequest {
    email: String,
    otp: String,
    #[serde(rename = "chainId")]
    chain_id: u64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MerchantEnrollResponse {
    #[serde(rename = "walletAddress")]
    pub wallet_address: String,
    pub email: String,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    pub s1k: Option<String>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct MerchantQuoteSignRequest {
    email: String,
    quote: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    s1k: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MerchantQuoteSignResponse {
    #[serde(rename = "walletAddress")]
    pub wallet_address: String,
    pub email: String,
    pub signature: Value,
}

impl ZkpayClient {
    pub fn new() -> Self {
        let base_url = std::env::var("ZPI_ZKPAY_URL")
            .unwrap_or_else(|_| "http://localhost:3002".into());
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Send an OTP to the merchant email.
    pub async fn send_otp(&self, email: &str) -> Result<SendOtpResponse, String> {
        let resp = self
            .client
            .post(format!("{}/petty-cash/send-otp", self.base_url))
            .json(&SendOtpRequest {
                email: email.into(),
            })
            .send()
            .await
            .map_err(|e| format!("Failed to reach zpi-zkpay: {}", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OTP request failed: {}", body));
        }

        resp.json::<SendOtpResponse>()
            .await
            .map_err(|e| format!("Failed to parse OTP response: {}", e))
    }

    /// Verify OTP and enroll the merchant wallet.
    pub async fn merchant_enroll(
        &self,
        email: &str,
        otp: &str,
        chain_id: u64,
    ) -> Result<MerchantEnrollResponse, String> {
        let resp = self
            .client
            .post(format!("{}/petty-cash/merchant-enroll", self.base_url))
            .json(&MerchantEnrollRequest {
                email: email.into(),
                otp: otp.into(),
                chain_id,
            })
            .send()
            .await
            .map_err(|e| format!("Failed to reach zpi-zkpay: {}", e))?;

        if !resp.status().is_success() {
            let body: String = resp.text().await.unwrap_or_default();
            // Try to parse as JSON error
            if let Ok(err) = serde_json::from_str::<ErrorResponse>(&body) {
                return Err(err.error);
            }
            return Err(format!("Enrollment failed: {}", body));
        }

        resp.json::<MerchantEnrollResponse>()
            .await
            .map_err(|e| format!("Failed to parse enrollment response: {}", e))
    }

    /// Request a Web3Auth-backed merchant quote signature.
    pub async fn sign_quote(
        &self,
        email: &str,
        quote: &Value,
        s1k: Option<&str>,
    ) -> Result<MerchantQuoteSignResponse, String> {
        let resp = self
            .client
            .post(format!("{}/petty-cash/sign-merchant-quote", self.base_url))
            .json(&MerchantQuoteSignRequest {
                email: email.into(),
                quote: quote.clone(),
                s1k: s1k.map(|v| v.to_string()),
            })
            .send()
            .await
            .map_err(|e| format!("Failed to reach zpi-zkpay: {}", e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            if let Ok(err) = serde_json::from_str::<ErrorResponse>(&body) {
                return Err(err.error);
            }
            return Err(format!("Quote signing failed: {}", body));
        }

        resp.json::<MerchantQuoteSignResponse>()
            .await
            .map_err(|e| format!("Failed to parse quote-sign response: {}", e))
    }
}

// ── On-chain balance queries ─────────────────────────────────────────────────

/// Chains the merchant wallet can operate on.
/// Same MPC address works on every EVM chain.
pub const SUPPORTED_CHAINS: &[(u64, &str)] = &[
    (11155111, "Sepolia"),
    (84532, "Base Sepolia"),
    (300, "ZKsync Era Sepolia"),
];

/// Per-chain balance snapshot.
#[derive(Serialize, Clone, Debug)]
pub struct ChainBalance {
    pub chain_id: u64,
    pub chain_name: String,
    pub eth: f64,
    pub usdc: f64,
}

/// Query ETH + USDC balances on all supported chains in parallel.
pub async fn get_all_balances(address: &str) -> Vec<ChainBalance> {
    let futs = SUPPORTED_CHAINS.iter().map(|&(cid, name)| {
        let addr = address.to_string();
        let chain_name = name.to_string();
        async move {
            let (eth, usdc) = tokio::join!(
                get_eth_balance(&addr, cid),
                get_usdc_balance(&addr, cid),
            );
            ChainBalance {
                chain_id: cid,
                chain_name,
                eth: eth.unwrap_or(0.0),
                usdc: usdc.unwrap_or(0.0),
            }
        }
    });
    futures::future::join_all(futs).await
}

/// RPC endpoints per chain.
fn rpc_url(chain_id: u64) -> String {
    match chain_id {
        11155111 => std::env::var("SEPOLIA_RPC_URL")
            .unwrap_or_else(|_| "https://ethereum-sepolia-rpc.publicnode.com".into()),
        84532 => std::env::var("BASE_SEPOLIA_RPC_URL")
            .unwrap_or_else(|_| "https://base-sepolia-rpc.publicnode.com".into()),
        300 => std::env::var("ZKSYNC_SEPOLIA_RPC_URL")
            .unwrap_or_else(|_| "https://sepolia.era.zksync.dev".into()),
        _ => "https://ethereum-sepolia-rpc.publicnode.com".into(),
    }
}

/// USDC contract addresses per chain.
pub fn usdc_address(chain_id: u64) -> &'static str {
    match chain_id {
        11155111 => "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238", // Sepolia USDC
        84532 => "0x036CbD53842c5426634e7929541eC2318f3dCF7e",   // Base Sepolia USDC
        300 => "0xAe045DE5638162fa134807Cb558E15A3F5A7F853",     // ZKsync Era Sepolia USDC
        _ => "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238",
    }
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Option<String>,
}

/// Fetch native ETH balance for an address via eth_getBalance.
pub async fn get_eth_balance(address: &str, chain_id: u64) -> Result<f64, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(rpc_url(chain_id))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getBalance",
            "params": [address, "latest"]
        }))
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {}", e))?;

    let rpc: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse RPC response: {}", e))?;

    let hex = rpc.result.ok_or("No result in RPC response")?;
    let wei = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| format!("Failed to parse hex balance: {}", e))?;
    Ok(wei as f64 / 1e18)
}

/// Fetch USDC balance (ERC-20) for an address via eth_call to balanceOf(address).
pub async fn get_usdc_balance(address: &str, chain_id: u64) -> Result<f64, String> {
    // balanceOf(address) selector = 0x70a08231, padded address
    let addr_padded = format!("{:0>64}", address.trim_start_matches("0x"));
    let data = format!("0x70a08231{}", addr_padded);

    let client = reqwest::Client::new();
    let resp = client
        .post(rpc_url(chain_id))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": usdc_address(chain_id), "data": data }, "latest"]
        }))
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {}", e))?;

    let rpc: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse RPC response: {}", e))?;

    let hex = rpc.result.ok_or("No result in RPC response")?;
    let raw = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| format!("Failed to parse hex balance: {}", e))?;
    // USDC has 6 decimals
    Ok(raw as f64 / 1e6)
}
