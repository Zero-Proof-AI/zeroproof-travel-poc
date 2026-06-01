use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
};
use base64::Engine as _;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;

use farm_core::cart::add_to_cart;
use farm_core::catalog::{find_product, list_products};
use farm_core::types::{Cart, Order, OrderStatus, PaymentMethod};

use super::db::SharedMerchantDb;
use super::enrollment::ZkpayClient;
use super::state::SharedFarmState;
use super::x402::{self, X402Config};

// ── Request / Response types ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListProductsRequest {
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GetProductRequest {
    pub product_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AddToCartRequest {
    pub session_id: String,
    pub product_id: String,
    #[serde(default = "default_quantity")]
    pub quantity: u32,
}

fn default_quantity() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
pub struct ViewCartRequest {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub session_id: String,
    #[serde(default = "default_payment_method")]
    pub payment_method: String,
}

fn default_payment_method() -> String {
    "x402_crypto".into()
}

#[derive(Debug, Deserialize)]
pub struct ClearCartRequest {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PayWithNeverminedRequest {
    pub merchant_url: String,
    pub amount: f64,
    pub description: String,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub zpi_proof: Option<String>,
    /// zk-attestation `zkp_proof_id` (e.g. `zkp-…`) returned by zpi_generate_zkp.
    /// When present, agent-b calls the attester's GET /proofs/{id}/verify
    /// endpoint before touching Nevermined. Without it, the merchant cannot
    /// independently validate the proof.
    #[serde(default)]
    pub zpi_proof_id: Option<String>,
    /// x402 access token minted by ZPI-ZKPay's `pay-with-nevermined-*` tool.
    /// When present, agent-b forwards it verbatim to Nevermined /verify and
    /// /settle instead of minting its own token from the user's NVM API key.
    /// The user's NVM API key never leaves ZPI-ZKPay on this path.
    #[serde(default)]
    pub x402_access_token: Option<String>,
    /// Optional x402 `payloadEncoded` from ZPI-ZKPay. Either this or
    /// `x402_access_token` is acceptable to Nevermined /verify; the
    /// facilitator treats them interchangeably.
    #[serde(default)]
    pub payload_encoded: Option<String>,
    /// Optional Nevermined plan id from ZPI-ZKPay's response. Required when
    /// `x402_access_token` is provided AND agent-b's NVM_API_KEY (if any)
    /// does not encode the same plan as the user's. ZPI-ZKPay returns
    /// `payment_required.accepts[0].planId` — Claude should forward that
    /// value here.
    #[serde(default)]
    pub plan_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VgsBillingAddress {
    pub line1: String,
    pub city: String,
    pub state: String,
    #[serde(rename = "postalCode")]
    pub postal_code: String,
    pub country: String,
}

#[derive(Debug, Deserialize)]
pub struct VgsBrowserData {
    #[serde(rename = "acceptHeader")]
    pub accept_header: String,
    #[serde(rename = "javaEnabled")]
    pub java_enabled: bool,
    #[serde(rename = "javascriptEnabled")]
    pub javascript_enabled: bool,
    pub language: String,
    #[serde(rename = "colorDepth")]
    pub color_depth: u32,
    #[serde(rename = "screenHeight")]
    pub screen_height: u32,
    #[serde(rename = "screenWidth")]
    pub screen_width: u32,
    #[serde(rename = "timeZone")]
    pub time_zone: i32,
    #[serde(rename = "userAgent")]
    pub user_agent: String,
}

#[derive(Debug, Deserialize)]
pub struct PayWithVgsCreditCardRequest {
    pub order_id: String,
    #[serde(default)]
    pub confirm_saved_profile: Option<bool>,
    #[serde(default)]
    pub card_holder_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub mobile_cc: Option<String>,
    #[serde(default)]
    pub mobile_subscriber: Option<String>,
    #[serde(default)]
    pub billing: Option<VgsBillingAddress>,
    #[serde(default)]
    pub browser: Option<VgsBrowserData>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub zpi_proof: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmVgsCreditCardPaymentRequest {
    pub order_id: String,
    #[serde(default = "default_true")]
    pub payment_confirmed: bool,
    #[serde(default)]
    pub transaction_ref: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub charge_bundle: Option<String>,
    #[serde(default)]
    pub zpi_response: Option<serde_json::Value>,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
pub struct FarmConfirmPaymentRequest {
    pub charge_bundle: String,
    pub external_id: String,
    pub merchant_id: String,
    #[serde(default)]
    pub psp_provider: Option<String>,
    #[serde(default)]
    pub psp_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TamperModeRequest {
    pub enabled: bool,
    pub multiplier: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct FarmToolResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_required: Option<serde_json::Value>,
}

impl FarmToolResponse {
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            status_code: None,
            payment_required: None,
        }
    }

    pub fn err(status: u16, error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            status_code: Some(status),
            payment_required: None,
        }
    }

    pub fn payment_required(payment_required: serde_json::Value, order_data: serde_json::Value) -> Self {
        Self {
            success: false,
            data: Some(order_data),
            error: None,
            status_code: Some(402),
            payment_required: Some(payment_required),
        }
    }
}

fn format_dollars(cents: u64) -> String {
    format!("${:.2}", cents as f64 / 100.0)
}

fn amount_to_cents(amount: f64) -> Result<u64, String> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err("amount must be a positive number".into());
    }
    Ok((amount * 100.0).round() as u64)
}

fn generate_external_id() -> String {
    format!("nm-ext-{}", uuid::Uuid::new_v4().to_string().replace('-', ""))
}

fn allowed_merchant_host(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|e| format!("merchant_url is invalid: {}", e))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "merchant_url must include a host".to_string())?
        .to_lowercase();

    let raw_allowlist = std::env::var("NEVERMINED_MERCHANT_ALLOWLIST")
        .unwrap_or_else(|_| "localhost,127.0.0.1".to_string());

    let allowed = raw_allowlist
        .split(',')
        .map(|h| h.trim().to_lowercase())
        .any(|h| !h.is_empty() && h == host);

    if allowed {
        Ok(())
    } else {
        Err(format!(
            "merchant_url host '{}' is not in NEVERMINED_MERCHANT_ALLOWLIST",
            host
        ))
    }
}

fn nevermined_http_trace_enabled() -> bool {
    std::env::var("NEVERMINED_HTTP_TRACE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn nevermined_verify_legacy_retry_enabled() -> bool {
    std::env::var("NEVERMINED_VERIFY_LEGACY_RETRY")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

fn redact_secret(value: &str) -> String {
    if value.is_empty() {
        return "<empty>".to_string();
    }
    if value.len() <= 10 {
        return "<redacted>".to_string();
    }
    let prefix = &value[..6];
    let suffix = &value[value.len().saturating_sub(4)..];
    format!("{}...{}", prefix, suffix)
}

fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let lower = k.to_ascii_lowercase();
                if lower.contains("token") || lower.contains("authorization") || lower.contains("api_key") {
                    let redacted = v
                        .as_str()
                        .map(redact_secret)
                        .map(serde_json::Value::String)
                        .unwrap_or_else(|| serde_json::Value::String("<redacted>".to_string()));
                    out.insert(k.clone(), redacted);
                } else {
                    out.insert(k.clone(), redact_json_value(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json_value).collect())
        }
        _ => value.clone(),
    }
}

fn default_nevermined_verify_url() -> String {
    match std::env::var("NVM_ENVIRONMENT")
        .unwrap_or_else(|_| "sandbox".to_string())
        .to_lowercase()
        .as_str()
    {
        "live" => "https://api.live.nevermined.app/api/v1/x402/verify".to_string(),
        _ => "https://api.sandbox.nevermined.app/api/v1/x402/verify".to_string(),
    }
}

async fn mint_nevermined_access_token(
    api_key: &str,
    amount_cents: u64,
    resource_url: &str,
) -> Result<String, String> {
    // NVM Pay flow: POST /access-token/from-nvm-key
    // You can override NEVERMINED_TOKEN_URL per environment if needed.
    let token_url = std::env::var("NEVERMINED_TOKEN_URL").ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://pay.nevermined.app/api/access-token/from-nvm-key".to_string());

    let max_attempts = std::env::var("NEVERMINED_TOKEN_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(3);
    let base_backoff_ms = std::env::var("NEVERMINED_TOKEN_RETRY_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 100)
        .unwrap_or(1000);

    let danger_tls = std::env::var("NEVERMINED_DANGER_ACCEPT_INVALID_CERTS")
        .ok()
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(danger_tls)
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;
    let amount = format!("{:.2}", amount_cents as f64 / 100.0);
    let pay_to = std::env::var("NEVERMINED_PAY_TO").unwrap_or_else(|_| "agent-b-farm".to_string());
    let mandate_id = std::env::var("NEVERMINED_MANDATE_ID").ok().filter(|v| !v.trim().is_empty());
    let scheme = std::env::var("NEVERMINED_SCHEME").ok().filter(|v| !v.trim().is_empty());
    let network = std::env::var("NEVERMINED_NETWORK").ok().filter(|v| !v.trim().is_empty());
    let plan_id = std::env::var("NEVERMINED_PLAN_ID").ok().filter(|v| !v.trim().is_empty());
    let agent_id_for_token = std::env::var("NEVERMINED_AGENT_ID").ok().filter(|v| !v.trim().is_empty());
    let http_trace = nevermined_http_trace_enabled();

    // Build request body once — cloned per retry attempt.
    // visa  (visa.nevermined.dev /api/access-token/from-nvm-key): { amount } only — backend resolves mandate from NVM key.
    // New API (api.{env}.nevermined.app /api/v1/x402/permissions): accepted / resource / delegationConfig.
    // Old API (pay.nevermined.app /api/access-token/from-nvm-key): amount / resource / payTo / mandateId.
    let request_body = match scheme.as_deref() {
        Some("visa") => {
            // Visa backend resolves mandate from the NVM API key — only amount is required.
            json!({ "amount": amount })
        }
        Some(scheme_val) => {
            // SDK body: { accepted: { scheme, network, planId, extra: { agentId } }, delegationConfig: { delegationId } }
            // No `resource` field — that's only in the PaymentRequired challenge, not the token request.
            let mut accepted = json!({
                "scheme": scheme_val,
                "network": network.as_deref().unwrap_or("stripe"),
            });
            if let Some(ref pid) = plan_id {
                accepted["planId"] = json!(pid);
            }
            if let Some(ref aid) = agent_id_for_token {
                accepted["extra"] = json!({ "agentId": aid });
            }
            let mut b = json!({ "accepted": accepted });
            if let Some(ref mid) = mandate_id {
                b["delegationConfig"] = json!({ "delegationId": mid });
            }
            b
        }
        None => {
            // Legacy format used by pay.nevermined.app/api/access-token/from-nvm-key
            let mut b = json!({
                "amount": amount,
                "resource": resource_url,
                "payTo": pay_to
            });
            if let Some(ref mid) = mandate_id {
                b["mandateId"] = json!(mid);
            }
            b
        }
    };

    // Nevermined NVM Pay expects the full prefixed key (e.g. "live:..." / "sandbox:...").
    let auth_api_key = api_key;

    let mut last_error: Option<String> = None;
    let mut attempts_made: usize = 0;

    for attempt in 1..=max_attempts {
        attempts_made = attempt;
        let body = request_body.clone();

        if http_trace {
            let redacted_body = redact_json_value(&body);
            tracing::info!(
                "[FARM-NVM][HTTP][REQUEST] method=POST url={} attempt={}/{} headers={{Authorization: Bearer {}, Content-Type: application/json}} body={}",
                token_url,
                attempt,
                max_attempts,
                redact_secret(auth_api_key),
                redacted_body
            );
        }

        let resp_result = client
            .post(&token_url)
            .header("Authorization", format!("Bearer {}", auth_api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

        match resp_result {
            Ok(resp) => {
                let status = resp.status();
                let raw_body = resp.text().await.unwrap_or_default();

                if http_trace {
                    let parsed = serde_json::from_str::<serde_json::Value>(&raw_body)
                        .map(|v| redact_json_value(&v).to_string())
                        .unwrap_or_else(|_| raw_body.clone());
                    tracing::info!(
                        "[FARM-NVM][HTTP][RESPONSE] method=POST url={} attempt={}/{} status={} body={}",
                        token_url,
                        attempt,
                        max_attempts,
                        status,
                        parsed
                    );
                }

                if status.is_success() {
                    let body = serde_json::from_str::<serde_json::Value>(&raw_body)
                        .map_err(|e| format!("failed to parse Nevermined token response: {}; raw={}", e, raw_body))?;

                    // NVM Pay responses vary by endpoint; accept common token field names.
                    // v2 x402 shape returns payloadEncoded (base64) — prefer that.
                    let token_val = body.get("payloadEncoded")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            body.get("accessToken")
                                .or_else(|| body.get("access_token"))
                                .or_else(|| body.get("token"))
                                .or_else(|| body.get("payment_token"))
                                .or_else(|| body.get("x402_token"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        });
                    return token_val.ok_or_else(|| {
                        format!(
                            "Nevermined token response did not include a token field. Response: {}",
                            body
                        )
                    });
                }

                let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
                last_error = Some(format!(
                    "Nevermined token exchange failed: status={} body={}",
                    status,
                    raw_body
                ));

                if retryable && attempt < max_attempts {
                    let delay = base_backoff_ms * attempt as u64;
                    tracing::warn!(
                        "[FARM-NVM] token endpoint retryable error (attempt {}/{}): status={} — retrying in {}ms",
                        attempt,
                        max_attempts,
                        status,
                        delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    continue;
                }

                break;
            }
            Err(e) => {
                let network_err = format!(
                    "failed to call Nevermined token endpoint {}: {} ({:?})",
                    token_url, e, e
                );
                last_error = Some(network_err.clone());
                if attempt < max_attempts {
                    let delay = base_backoff_ms * attempt as u64;
                    tracing::warn!(
                        "[FARM-NVM] token endpoint network error (attempt {}/{}): {} — retrying in {}ms",
                        attempt,
                        max_attempts,
                        network_err,
                        delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    continue;
                }
                break;
            }
        }
    }

    Err(format!(
        "Nevermined token endpoint unavailable after {} attempt(s). {}. You can retry shortly or switch to x402_crypto.",
        attempts_made,
        last_error.unwrap_or_else(|| "unknown error".to_string())
    ))
}

/// Extract the `jti` (plan ID) from a Nevermined API key JWT.
/// The key has the form `sandbox:header.payload.sig` or plain `header.payload.sig`.
fn extract_nvm_plan_id(nvm_api_key: &str) -> Option<String> {
    // Strip optional "sandbox:" / "live:" prefix
    let jwt_part = nvm_api_key.splitn(2, ':').last().unwrap_or(nvm_api_key);
    let payload_b64 = jwt_part.splitn(3, '.').nth(1)?;
    // Base64url decode (no padding)
    let padded = match payload_b64.len() % 4 {
        2 => format!("{}==", payload_b64),
        3 => format!("{}=", payload_b64),
        _ => payload_b64.to_string(),
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&padded))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v.get("jti").and_then(|j| j.as_str()).map(|s| s.to_string())
}

/// Decode a base64(url) x402 access token envelope and pull out `accepted.planId`.
///
/// The envelope is `{ x402Version, accepted: { scheme, network, planId, extra },
/// payload: { token: <inner_jwt> } }`. Used when ZPI-ZKPay supplies the token
/// but no explicit plan_id — the merchant's NVM key (if any) charges against
/// the wrong plan in this flow, so the token's own planId is authoritative.
fn extract_plan_id_from_x402_token(token: &str) -> Option<String> {
    let padded = {
        let n = token.len();
        let m = n % 4;
        if m == 0 { token.to_string() } else { format!("{}{}", token, "=".repeat(4 - m)) }
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&padded))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&padded))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v.get("accepted")
        .and_then(|a| a.get("planId"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string())
}

/// Resolve the bearer to send on /x402/verify and /x402/settle.
///
/// Nevermined sandbox does not validate the Authorization header on either
/// endpoint — even "no header" and "garbage" returned the same status as the
/// control. The token in the body is what gets validated. We send a bearer
/// anyway, in priority order:
///   1. NVM_MERCHANT_API_KEY     (merchant's own NVM key — preferred name)
///   2. NVM_API_KEY              (legacy fallback for existing deployments)
///   3. the x402 access token    (semantic match for the partnership-proposal
///                                flow; user's NVM key never required)
///
/// This keeps existing demos working while letting agent-b operate without
/// any NVM API key once the env var is removed from .env.
fn resolve_merchant_bearer(token: &str) -> String {
    if let Ok(v) = std::env::var("NVM_MERCHANT_API_KEY") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if let Ok(v) = std::env::var("NVM_API_KEY") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    token.to_string()
}

async fn verify_nevermined_token_if_configured(
    token: &str,
    amount_cents: u64,
    resource_url: &str,
) -> Result<(), String> {
    let verify_url = match std::env::var("NEVERMINED_VERIFY_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => default_nevermined_verify_url(),
    };

    let api_key = resolve_merchant_bearer(token);

    // NEVERMINED_VERIFY_SCHEMA selects the request body format:
    //   "facilitator" — legacy facilitator.nevermined.app schema:
    //                   { paymentRequired: {...}, x402AccessToken: "<token>" }
    //   "api"         — latest Nevermined API docs (api.live.nevermined.app):
    //                   { accessToken: "<token>", agentId: "did:nv:...", creditsRequested: N }
    let schema = std::env::var("NEVERMINED_VERIFY_SCHEMA")
        .unwrap_or_else(|_| "facilitator".to_string())
        .to_lowercase();

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(
            std::env::var("NEVERMINED_DANGER_ACCEPT_INVALID_CERTS")
                .ok()
                .map(|v| v.trim().eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        )
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;
    let http_trace = nevermined_http_trace_enabled();

    // Visa path: send token as PAYMENT-SIGNATURE header — no JSON body, no Authorization.
    if schema == "visa" {
        if http_trace {
            tracing::info!(
                "[FARM-NVM][HTTP][REQUEST] method=POST url={} schema=visa headers={{PAYMENT-SIGNATURE: [redacted]}}",
                verify_url
            );
        }
        let resp = client
            .post(&verify_url)
            .header("PAYMENT-SIGNATURE", token)
            .send()
            .await
            .map_err(|e| format!("failed to verify Visa payment at {}: {}", verify_url, e))?;
        let status = resp.status();
        let raw_body = resp.text().await.unwrap_or_default();
        if http_trace {
            tracing::info!(
                "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=visa status={} body={}",
                verify_url, status, raw_body
            );
        }
        if status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
            tracing::info!("Nevermined Visa verify response: {}", body);
            return Ok(());
        }
        return Err(format!("Nevermined Visa verify {} ({})", status, raw_body));
    }

    let req = client
        .post(&verify_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json");

    let verify_body = if schema == "api" {
        // ✅ LATEST API DOC (https://nevermined.ai/docs/api-reference/x402/verify-permission)
        let agent_id = std::env::var("NEVERMINED_AGENT_ID")
            .unwrap_or_else(|_| "did:nv:agent-b-farm".to_string());
        let credits_requested = amount_cents as i64;
        tracing::debug!(
            "Nevermined verify [api schema] to {}: agentId={} creditsRequested={}",
            verify_url, agent_id, credits_requested
        );
        json!({
            "accessToken": token,
            "agentId": agent_id,
            "creditsRequested": credits_requested
        })
    } else {
        // Legacy facilitator schema: { paymentRequired, x402AccessToken }
        let scheme = std::env::var("NEVERMINED_SCHEME").unwrap_or_else(|_| "nvm:erc4337".to_string());
        let network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| {
            if scheme == "nvm:card-delegation" { "stripe".to_string() } else { "eip155:84532".to_string() }
        });
        let plan_id = std::env::var("NEVERMINED_PLAN_ID").ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| std::env::var("NVM_API_KEY").ok().and_then(|k| extract_nvm_plan_id(&k)))
            .unwrap_or_default();
        let payment_required = json!({
            "x402Version": 2,
            "resource": { "url": resource_url },
            "accepts": [{ "scheme": scheme, "network": network, "planId": plan_id, "extra": { "version": "1" } }],
            "extensions": {}
        });
        tracing::debug!(
            "Nevermined verify [facilitator schema] to {}: scheme={} network={} planId={} token_len={}",
            verify_url, scheme, network, plan_id, token.len()
        );
        json!({
            "paymentRequired": payment_required,
            "x402AccessToken": token
        })
    };

    if http_trace {
        let redacted_body = redact_json_value(&verify_body);
        tracing::info!(
            "[FARM-NVM][HTTP][REQUEST] method=POST url={} schema={} headers={{Authorization: Bearer {}, Content-Type: application/json}} body={}",
            verify_url,
            schema,
            redact_secret(&api_key),
            redacted_body
        );
    }

    let resp = req
        .json(&verify_body)
        .send()
        .await
        .map_err(|e| format!("failed to verify Nevermined credential at {}: {}", verify_url, e))?;

    let status = resp.status();
    let raw_body = resp.text().await.unwrap_or_default();

    if http_trace {
        let parsed = serde_json::from_str::<serde_json::Value>(&raw_body)
            .map(|v| redact_json_value(&v).to_string())
            .unwrap_or_else(|_| raw_body.clone());
        tracing::info!(
            "[FARM-NVM][HTTP][RESPONSE] method=POST url={} status={} body={}",
            verify_url,
            status,
            parsed
        );
    }

    if status.is_success() {
        let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
        tracing::info!("Nevermined verify response: {}", body);
        return Ok(());
    }

    // Some live deployments still validate legacy payload fields on /x402/verify.
    // If api schema is rejected with missing `paymentRequired`/`x402AccessToken`, retry once.
    let looks_like_legacy_validation = raw_body.contains("paymentRequired should not be null or undefined")
        || raw_body.contains("x402AccessToken should not be null or undefined")
        || raw_body.contains("x402AccessToken must be a string");

    if schema == "api"
        && status.as_u16() == 400
        && looks_like_legacy_validation
        && nevermined_verify_legacy_retry_enabled()
    {
        let scheme = std::env::var("NEVERMINED_SCHEME").unwrap_or_else(|_| "nvm:erc4337".to_string());
        let network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| {
            if scheme == "nvm:card-delegation" { "stripe".to_string() } else { "eip155:84532".to_string() }
        });
        let plan_id = std::env::var("NEVERMINED_PLAN_ID").ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| std::env::var("NVM_API_KEY").ok().and_then(|k| extract_nvm_plan_id(&k)))
            .unwrap_or_default();
        let payment_required = json!({
            "x402Version": 2,
            "resource": { "url": resource_url },
            "accepts": [{ "scheme": scheme, "network": network, "planId": plan_id, "extra": { "version": "1" } }],
            "extensions": {}
        });
        let fallback_body = json!({
            "paymentRequired": payment_required,
            "x402AccessToken": token
        });

        tracing::warn!(
            "Nevermined verify api schema was rejected with legacy validation error; retrying once with legacy schema at {}",
            verify_url
        );

        if http_trace {
            let redacted_body = redact_json_value(&fallback_body);
            tracing::info!(
                "[FARM-NVM][HTTP][REQUEST] method=POST url={} schema=legacy-fallback headers={{Authorization: Bearer {}, Content-Type: application/json}} body={}",
                verify_url,
                redact_secret(&api_key),
                redacted_body
            );
        }

        let fallback_resp = client
            .post(&verify_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&fallback_body)
            .send()
            .await
            .map_err(|e| format!("failed legacy fallback verify at {}: {}", verify_url, e))?;

        let fallback_status = fallback_resp.status();
        let fallback_raw_body = fallback_resp.text().await.unwrap_or_default();

        if http_trace {
            let parsed = serde_json::from_str::<serde_json::Value>(&fallback_raw_body)
                .map(|v| redact_json_value(&v).to_string())
                .unwrap_or_else(|_| fallback_raw_body.clone());
            tracing::info!(
                "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=legacy-fallback status={} body={}",
                verify_url,
                fallback_status,
                parsed
            );
        }

        if fallback_status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&fallback_raw_body).unwrap_or_default();
            tracing::info!("Nevermined verify response (legacy fallback): {}", body);
            return Ok(());
        }

        return Err(format!(
            "Nevermined verify failed api={} ({}) legacy-fallback={} ({})",
            status,
            raw_body,
            fallback_status,
            fallback_raw_body
        ));
    }

    Err(format!("Nevermined facilitator {} ({})", status, raw_body))
}

async fn settle_nevermined_token(
    token: &str,
    amount_cents: u64,
    resource_url: &str,
    plan_id_override: Option<&str>,
) -> Result<Option<String>, String> {
    let settle_url = match std::env::var("NEVERMINED_SETTLE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => match std::env::var("NVM_ENVIRONMENT")
            .unwrap_or_else(|_| "sandbox".to_string())
            .to_lowercase()
            .as_str()
        {
            "live" => "https://api.live.nevermined.app/api/v1/x402/settle".to_string(),
            _ => "https://api.sandbox.nevermined.app/api/v1/x402/settle".to_string(),
        },
    };

    let api_key = resolve_merchant_bearer(token);

    let scheme = std::env::var("NEVERMINED_SCHEME").unwrap_or_default();
    let http_trace = nevermined_http_trace_enabled();
    let danger_tls = std::env::var("NEVERMINED_DANGER_ACCEPT_INVALID_CERTS")
        .ok()
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(danger_tls)
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    // Visa path: send token as PAYMENT-SIGNATURE header — no JSON body, no Authorization.
    if scheme == "visa" {
        if http_trace {
            tracing::info!(
                "[FARM-NVM][HTTP][REQUEST] method=POST url={} schema=visa headers={{PAYMENT-SIGNATURE: [redacted]}}",
                settle_url
            );
        }
        let resp = client
            .post(&settle_url)
            .header("PAYMENT-SIGNATURE", token)
            .send()
            .await
            .map_err(|e| format!("failed to call Visa settle at {}: {}", settle_url, e))?;
        let status = resp.status();
        let raw_body = resp.text().await.unwrap_or_default();
        if http_trace {
            tracing::info!(
                "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=visa status={} body={}",
                settle_url, status, raw_body
            );
        }
        if status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
            tracing::info!("Nevermined Visa settle response: {}", body);
            let tx = body.get("transaction").and_then(|v| v.as_str()).map(|s| s.to_string());
            return Ok(tx);
        }
        return Err(format!("Nevermined Visa settle {} ({})", status, raw_body));
    }

    let credits_used = amount_cents as i64;

    let network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| {
        if scheme == "nvm:card-delegation" { "stripe".to_string() } else { "eip155:84532".to_string() }
    });
    // Priority: explicit override (from ZPI-ZKPay's response) → env var → jti
    // of the merchant's NVM key. In the new architecture the override is the
    // user's planId (the token charges against the user's delegation, not
    // the merchant's). Falling back to the merchant's key produces the
    // wrong planId when the two keys differ.
    let plan_id = plan_id_override
        .map(|s| s.to_string())
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("NEVERMINED_PLAN_ID").ok().filter(|v| !v.trim().is_empty())
        })
        .or_else(|| std::env::var("NVM_API_KEY").ok().and_then(|k| extract_nvm_plan_id(&k)))
        .unwrap_or_default();

    let payment_required = json!({
        "x402Version": 2,
        "resource": { "url": resource_url },
        "accepts": [{ "scheme": scheme, "network": network, "planId": plan_id, "extra": { "version": "1" } }],
        "extensions": {}
    });

    let body = json!({
        "paymentRequired": payment_required,
        "x402AccessToken": token,
        "creditsUsed": credits_used
    });

    if http_trace {
        let redacted_body = redact_json_value(&body);
        tracing::info!(
            "[FARM-NVM][HTTP][REQUEST] method=POST url={} schema=facilitator headers={{Authorization: Bearer {}, Content-Type: application/json}} body={}",
            settle_url,
            redact_secret(&api_key),
            redacted_body
        );
    }

    tracing::info!(
        "[FARM-NVM][SETTLE][BODY] url={} scheme={} network={} planId={} creditsUsed={} token={} paymentRequired={}",
        settle_url, scheme, network, plan_id, credits_used,
        redact_secret(token),
        serde_json::to_string(&payment_required).unwrap_or_default()
    );
    let resp = client
        .post(&settle_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("failed to call Nevermined settle at {}: {}", settle_url, e))?;

    let status = resp.status();
    let raw_body = resp.text().await.unwrap_or_default();

    if http_trace {
        let parsed = serde_json::from_str::<serde_json::Value>(&raw_body)
            .map(|v| redact_json_value(&v).to_string())
            .unwrap_or_else(|_| raw_body.clone());
        tracing::info!(
            "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=facilitator status={} body={}",
            settle_url,
            status,
            parsed
        );
    }

    if status.is_success() {
        let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
        tracing::info!("Nevermined settle response: {}", body);
        let tx_hash = body.get("txHash").and_then(|v| v.as_str()).map(|s| s.to_string());
        Ok(tx_hash)
    } else {
        Err(format!("Nevermined settle {} ({})", status, raw_body))
    }
}

/// Default attester base URL when `ZPI_ATTESTER_URL` is unset.
fn default_zpi_attester_url() -> String {
    std::env::var("ZPI_ATTESTER_URL").unwrap_or_else(|_| "http://localhost:8000".to_string())
}

/// Verify a ZPI proof against the zk-attestation-service.
///
/// Before settling, the merchant pulls the proof from the attester and
/// verifies it. Without this check, any opaque blob Claude forwards as
/// "proof" would pass the gate.
///
/// Behaviour:
///   * If `zpi_proof_id` is `None` or empty → returns Ok (soft-skip). We
///     warn so the missing proof shows up in the log, but we don't block —
///     existing demos that haven't been updated to pass `zpi_proof_id` keep
///     working.
///   * If `zpi_proof_id` is present → GET `{ZPI_ATTESTER_URL}/proofs/{id}/verify`.
///     A 2xx with `{"success": true}` is the only accepted outcome.
///   * If the supplied `zpi_proof_id` 404s, fall back to looking up the proof
///     by `external_id` via `GET /proofs/session/{external_id}`. The
///     zpi-cli MCP server currently fabricates a local `zkp-<uuid>` id and
///     returns it as `zkp_proof_id`, while the attester stores the proof
///     under its own UUID `proof_id`. Once the CLI is updated to surface the
///     attester's id, this fallback becomes unreachable.
async fn verify_zpi_proof_against_attester(
    zpi_proof_id: Option<&str>,
    external_id: &str,
    expected_amount_cents: u64,
    expected_merchant_url: &str,
) -> Result<(), String> {
    let supplied_id = match zpi_proof_id.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            tracing::warn!(
                "[FARM-NVM][ZPI] No zpi_proof_id supplied for external_id={} amount_cents={} merchant_url={} — skipping attester verification. The merchant cannot independently verify the ZPI intent proof. Update ZPI-ZKPay to pass zpi_proof_id through.",
                external_id,
                expected_amount_cents,
                expected_merchant_url
            );
            return Ok(());
        }
    };

    let attester_url = default_zpi_attester_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client for attester: {}", e))?;

    match verify_attester_proof_by_id(&client, &attester_url, supplied_id).await {
        AttesterVerifyOutcome::Ok { session_id } => {
            tracing::info!(
                "[FARM-NVM][ZPI] ✅ proof verified by attester proof_id={} session_id={} external_id={}",
                supplied_id,
                session_id.as_deref().unwrap_or("<none>"),
                external_id
            );
            Ok(())
        }
        AttesterVerifyOutcome::NotFound => {
            tracing::warn!(
                "[FARM-NVM][ZPI] supplied zpi_proof_id={} not recognised by the attester — \
                 falling back to lookup by external_id={}. This usually means the zpi-cli \
                 returned its local `zkp-<uuid>` id rather than the attester's UUID.",
                supplied_id,
                external_id
            );
            let resolved =
                resolve_attester_proof_id_by_external_id(&client, &attester_url, external_id)
                    .await?;
            match verify_attester_proof_by_id(&client, &attester_url, &resolved).await {
                AttesterVerifyOutcome::Ok { session_id } => {
                    tracing::info!(
                        "[FARM-NVM][ZPI] ✅ proof verified by attester (via external_id fallback) proof_id={} session_id={} external_id={} supplied_id={}",
                        resolved,
                        session_id.as_deref().unwrap_or("<none>"),
                        external_id,
                        supplied_id
                    );
                    Ok(())
                }
                AttesterVerifyOutcome::NotFound => Err(format!(
                    "[FARM-NVM][ZPI] resolved proof_id={} (from external_id={}) was not found by the attester after lookup",
                    resolved, external_id
                )),
                AttesterVerifyOutcome::Failed(msg) => Err(msg),
            }
        }
        AttesterVerifyOutcome::Failed(msg) => Err(msg),
    }
}

enum AttesterVerifyOutcome {
    Ok { session_id: Option<String> },
    NotFound,
    Failed(String),
}

async fn verify_attester_proof_by_id(
    client: &reqwest::Client,
    attester_url: &str,
    proof_id: &str,
) -> AttesterVerifyOutcome {
    let verify_url = format!(
        "{}/proofs/{}/verify",
        attester_url.trim_end_matches('/'),
        proof_id
    );

    tracing::info!(
        "[FARM-NVM][ZPI] Verifying proof against attester proof_id={} url={}",
        proof_id,
        verify_url
    );

    let resp = match client.get(&verify_url).send().await {
        Ok(r) => r,
        Err(e) => {
            return AttesterVerifyOutcome::Failed(format!(
                "[FARM-NVM][ZPI] proof verification request failed proof_id={} err={}",
                proof_id, e
            ));
        }
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::NOT_FOUND {
        return AttesterVerifyOutcome::NotFound;
    }

    if !status.is_success() {
        return AttesterVerifyOutcome::Failed(format!(
            "[FARM-NVM][ZPI] attester returned HTTP {} for proof_id={}: {}",
            status, proof_id, body
        ));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return AttesterVerifyOutcome::Failed(format!(
                "[FARM-NVM][ZPI] attester response was not JSON proof_id={} err={} body={}",
                proof_id, e, body
            ));
        }
    };

    let success = parsed
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !success {
        let error_msg = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // The attester returns 200 + {"success": false} when the proof
        // record doesn't exist, so treat "Proof not found" as NotFound
        // so the caller can fall back to looking up by external_id.
        if error_msg.to_lowercase().contains("not found") {
            return AttesterVerifyOutcome::NotFound;
        }
        return AttesterVerifyOutcome::Failed(format!(
            "[FARM-NVM][ZPI] proof verification failed proof_id={} reason={}",
            proof_id,
            if error_msg.is_empty() {
                "attester reported success=false"
            } else {
                error_msg
            }
        ));
    }

    let session_id = parsed
        .pointer("/data/proof/session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    AttesterVerifyOutcome::Ok { session_id }
}

async fn resolve_attester_proof_id_by_external_id(
    client: &reqwest::Client,
    attester_url: &str,
    external_id: &str,
) -> Result<String, String> {
    let list_url = format!(
        "{}/proofs/session/{}",
        attester_url.trim_end_matches('/'),
        external_id
    );

    let resp = client.get(&list_url).send().await.map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] attester session lookup failed external_id={} err={}",
            external_id, e
        )
    })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(format!(
            "[FARM-NVM][ZPI] attester returned HTTP {} for session lookup external_id={}: {}",
            status, external_id, body
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] attester session response was not JSON external_id={} err={} body={}",
            external_id, e, body
        )
    })?;

    let proofs = parsed
        .get("proofs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            format!(
                "[FARM-NVM][ZPI] attester session response missing `proofs` array external_id={} body={}",
                external_id, body
            )
        })?;

    // Pick the most recent attest-tool entry. The attester writes its own
    // saved proofs with `tool_name = "attest"`; verified proofs sit there.
    let candidate = proofs
        .iter()
        .filter(|p| {
            p.get("tool_name")
                .and_then(|v| v.as_str())
                .map(|s| s == "attest")
                .unwrap_or(true)
        })
        .max_by_key(|p| p.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0))
        .or_else(|| proofs.last())
        .ok_or_else(|| {
            format!(
                "[FARM-NVM][ZPI] no proofs found at attester for external_id={}",
                external_id
            )
        })?;

    let resolved = candidate
        .get("proof_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "[FARM-NVM][ZPI] attester proof entry missing `proof_id` external_id={} entry={}",
                external_id, candidate
            )
        })?
        .to_string();

    Ok(resolved)
}

pub async fn handle_pay_with_nevermined(
    State(state): State<SharedFarmState>,
    Json(req): Json<PayWithNeverminedRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    allowed_merchant_host(&req.merchant_url).map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(FarmToolResponse::err(403, e)),
        )
    })?;

    let amount_cents = amount_to_cents(req.amount).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, e)),
        )
    })?;

    let requested_external_id = req.external_id.clone();
    let zpi_proof = req.zpi_proof.clone().unwrap_or_default();

    // The preferred flow has Claude forward an x402_access_token that ZPI-ZKPay
    // already minted on the user's behalf. In that case the raw zpi_proof bytes
    // are not required — the proof's validity is established by zpi_proof_id
    // against the attester. The legacy flow still requires the bytes so this
    // handler can stash pending state and mint locally using NVM_API_KEY.
    let has_supplied_token = req
        .x402_access_token
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || req
            .payload_encoded
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

    if zpi_proof.trim().is_empty() && !has_supplied_token {
        let mut state = state.write().await;
        let external_id = requested_external_id.unwrap_or_else(generate_external_id);
        state.pending_nevermined.insert(
            external_id.clone(),
            super::state::PendingNeverminedPayment {
                merchant_url: req.merchant_url.clone(),
                amount_cents,
                description: req.description.clone(),
            },
        );

        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "NEEDS_INTENT_PROOF",
            "external_id": external_id,
            "intent_type": "spend",
            "payment_details": {
                "amount": format_dollars(amount_cents),
                "amount_cents": amount_cents,
                "currency": "USD",
                "merchant_url": req.merchant_url,
                "description": req.description,
                "payment_processor": "nevermined"
            },
            "instructions": "PREFERRED FLOW: call zpi-zkpay's pay-with-nevermined-merchant-settles first (it mints the x402 token on the user's behalf without leaking the user's NVM API key to the merchant), then call this tool with x402_access_token + plan_id + zpi_proof_id. LEGACY: call chp_save, then prove_intent with this external_id and intent_type='spend', then call pay-with-nevermined again with zpi_proof + external_id (this minted token will come from the merchant's own NVM_API_KEY)."
        }))));
    }

    if !has_supplied_token && zpi_proof.trim().len() < 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "zpi_proof looks invalid (too short)".to_string(),
            )),
        ));
    }

    let external_id = requested_external_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "external_id is required".to_string(),
            )),
        )
    })?;

    // The pending-state check is only meaningful for the legacy path: agent-b
    // stashed merchant_url + amount in its NEEDS_INTENT_PROOF response, then
    // verifies the second call matches. In the new flow ZPI-ZKPay holds the
    // pending state and the attester's field commitments are the cryptographic
    // binding between the proof and the amount/merchant_url, so the local
    // record is unavailable and unnecessary.
    if !has_supplied_token {
        let pending = {
            let state = state.read().await;
            state.pending_nevermined.get(&external_id).cloned()
        }
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    format!("No pending Nevermined payment for external_id={}", external_id),
                )),
            )
        })?;

        if pending.amount_cents != amount_cents || pending.merchant_url != req.merchant_url {
            let new_external_id = generate_external_id();
            let mut state = state.write().await;
            state.pending_nevermined.remove(&external_id);
            state.pending_nevermined.insert(
                new_external_id.clone(),
                super::state::PendingNeverminedPayment {
                    merchant_url: req.merchant_url.clone(),
                    amount_cents,
                    description: req.description.clone(),
                },
            );

            return Ok(Json(FarmToolResponse::ok(json!({
                "status": "PROOF_MISMATCH",
                "reason": "merchant_url or amount changed from the pending intent proof request",
                "action": "STOP — ask user to confirm and regenerate proof with the new_external_id",
                "new_external_id": new_external_id,
                "proof_amount": format_dollars(pending.amount_cents),
                "required_amount": format_dollars(amount_cents)
            }))));
        }
    }

    // ── Phase 2a — verify the ZPI proof against the attester ──────────────
    // In the preferred flow zpi_proof_id is mandatory — the whole point is
    // that the merchant verifies the proof itself rather than trusting Claude
    // or ZPI-ZKPay blindly. In the legacy flow we soft-skip if it's missing
    // so older demos still work (with a visible warning emitted downstream).
    let zpi_proof_id_present = req
        .zpi_proof_id
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    if has_supplied_token && !zpi_proof_id_present {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "zpi_proof_id is required when x402_access_token is supplied so the merchant can verify the ZPI intent proof against the attester".to_string(),
            )),
        ));
    }

    verify_zpi_proof_against_attester(
        req.zpi_proof_id.as_deref(),
        &external_id,
        amount_cents,
        &req.merchant_url,
    )
    .await
    .map_err(|e| {
        tracing::error!("{}", e);
        (
            StatusCode::FORBIDDEN,
            Json(FarmToolResponse::err(403, e)),
        )
    })?;

    // ── Resolve the payment credential ───────────────────────────────────
    // ZPI-ZKPay owns /x402/permissions (it holds the user's NVM API key).
    // When `x402_access_token` is supplied, use it verbatim. When it's missing,
    // fall back to the legacy path (mint locally using NVM_API_KEY) so older
    // demos that haven't been updated keep working — emit a visible warning.
    let supplied_token = req
        .x402_access_token
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            req.payload_encoded
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    let (payment_credential, x_nevermined_api_key) = match supplied_token {
        Some(token) => {
            tracing::info!(
                "[FARM-NVM] Using x402_access_token supplied by ZPI-ZKPay (mint skipped) external_id={} token={}",
                external_id,
                redact_secret(&token)
            );
            (token, String::new())
        }
        None => {
            tracing::warn!(
                "[FARM-NVM] No x402_access_token supplied — falling back to legacy mint via NVM_API_KEY. \
                 Update Claude's flow to call zpi-zkpay's pay-with-nevermined-merchant-settles first \
                 and forward x402_access_token + plan_id here. external_id={}",
                external_id
            );
            let nvm_api_key = std::env::var("NVM_API_KEY").map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(FarmToolResponse::err(
                        500,
                        "x402_access_token is missing and NVM_API_KEY is not set — cannot proceed".to_string(),
                    )),
                )
            })?;
            let use_token_exchange = std::env::var("NEVERMINED_USE_TOKEN_EXCHANGE")
                .ok()
                .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
                .unwrap_or(true);
            let cred = if use_token_exchange {
                mint_nevermined_access_token(&nvm_api_key, amount_cents, &req.merchant_url)
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(FarmToolResponse::err(502, e)),
                        )
                    })?
            } else {
                nvm_api_key.clone()
            };
            (cred, nvm_api_key)
        }
    };

    verify_nevermined_token_if_configured(&payment_credential, amount_cents, &req.merchant_url)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(FarmToolResponse::err(502, e)),
            )
        })?;

    let stop_after_verify = std::env::var("NEVERMINED_STOP_AFTER_VERIFY")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    if stop_after_verify {
        tracing::error!(
            "[FARM-NVM] NEVERMINED_STOP_AFTER_VERIFY enabled; stopping before merchant handoff external_id={} merchant_url={} credential={}",
            external_id,
            req.merchant_url,
            payment_credential
        );

        return Err((
            StatusCode::PRECONDITION_FAILED,
            Json(FarmToolResponse::err(
                412,
                json!({
                    "status": "STOPPED_AFTER_VERIFY",
                    "reason": "Configured to stop after Nevermined returned the payment credential and before merchant charge submission.",
                    "external_id": external_id,
                    "merchant_url": req.merchant_url,
                    "payment_processor": "nevermined",
                    "payment_credential": payment_credential,
                })
                .to_string(),
            )),
        ));
    }

    // Resolve the planId for the internal checkout call. Priority:
    // 1. Explicit plan_id from the request body (ZPI-ZKPay echoes the user's planId in its response).
    // 2. The x402 token's own `accepted.planId` field.
    // 3. Empty — the checkout endpoint falls back to env-driven defaults.
    let plan_id_for_forward = req
        .plan_id
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| extract_plan_id_from_x402_token(&payment_credential))
        .unwrap_or_default();

    let client = reqwest::Client::new();
    let mut merchant_req = client
        .get(&req.merchant_url)
        .header("Authorization", format!("Bearer {}", payment_credential))
        .header("X-Nevermined-Access-Token", &payment_credential)
        .header("X-Expected-Amount-Cents", amount_cents.to_string())
        .header("X-ZPI-External-Id", &external_id);
    if !x_nevermined_api_key.is_empty() {
        // Only forward the merchant-side key on the legacy path so the
        // checkout endpoint can preserve its existing behaviour.
        merchant_req = merchant_req.header("X-Nevermined-Api-Key", &x_nevermined_api_key);
    }
    if !plan_id_for_forward.is_empty() {
        merchant_req = merchant_req.header("X-Nevermined-Plan-Id", &plan_id_for_forward);
    }
    let merchant_resp = merchant_req
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(FarmToolResponse::err(
                    502,
                    format!("failed to call merchant_url: {}", e),
                )),
            )
        })?;

    let status = merchant_resp.status();
    let text = merchant_resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "PAYMENT_FAILED",
            "reason": format!("merchant returned {}", status),
            "merchant_response": text
        }))));
    }

    {
        let mut state = state.write().await;
        state.pending_nevermined.remove(&external_id);
    }

    Ok(Json(FarmToolResponse::ok(json!({
        "status": "PAID",
        "payment_processor": "nevermined",
        "external_id": external_id,
        "merchant_response": serde_json::from_str::<serde_json::Value>(&text).unwrap_or(json!({"raw": text}))
    }))))
}

pub async fn handle_checkout_with_credit_card(
    State(state): State<SharedFarmState>,
    Json(req): Json<PayWithVgsCreditCardRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!(
        "[FARM-VGS] checkout-with-credit-card called: order_id={}, external_id={}, has_zpi_proof={}, confirm_saved_profile={}",
        req.order_id,
        req.external_id.as_deref().unwrap_or("<auto>"),
        req.zpi_proof.as_ref().map(|p| !p.trim().is_empty()).unwrap_or(false),
        req.confirm_saved_profile.unwrap_or(false),
    );

    let zpi_proof = req.zpi_proof.clone().unwrap_or_default();
    let external_id = req
        .external_id
        .clone()
        .unwrap_or_else(|| format!("vgs-ext-{}", req.order_id));

    if zpi_proof.trim().is_empty() {
        tracing::info!(
            "[FARM-VGS] checkout-with-credit-card returning NEEDS_INTENT_PROOF: order_id={}, external_id={}",
            req.order_id,
            external_id
        );
        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "NEEDS_INTENT_PROOF",
            "order_id": req.order_id,
            "external_id": external_id,
            "intent_type": "spend",
            "payment_processor": "vgs_card",
            "instructions": "Call chp_save, then prove_intent with this external_id and intent_type='spend', then call checkout-with-credit-card again with zpi_proof."
        }))));
    }

    if zpi_proof.trim().len() < 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "zpi_proof looks invalid (too short)".to_string(),
            )),
        ));
    }

    let order = {
        let state = state.read().await;
        state.orders.get(&req.order_id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(FarmToolResponse::err(
                404,
                format!("Order '{}' not found", req.order_id),
            )),
        )
    })?;

    if order.status == OrderStatus::Paid {
        return Err((
            StatusCode::CONFLICT,
            Json(FarmToolResponse::err(409, "Order already paid".into())),
        ));
    }

    let merchant_id = std::env::var("VGS_MERCHANT_ID")
        .unwrap_or_else(|_| "test-merchant-p2".to_string());

    let mut body_obj = serde_json::Map::new();
    body_obj.insert("amount".to_string(), json!(order.total_cents));
    body_obj.insert("currency".to_string(), json!("840"));
    body_obj.insert("merchant_id".to_string(), json!(merchant_id));
    body_obj.insert("external_id".to_string(), json!(external_id));
    if let Some(v) = req.confirm_saved_profile {
        body_obj.insert("confirm_profile".to_string(), json!(v));
    }

    if let Some(v) = req.card_holder_name.as_ref().filter(|s| !s.trim().is_empty()) {
        body_obj.insert("card_holder_name".to_string(), json!(v));
    }
    if let Some(v) = req.email.as_ref().filter(|s| !s.trim().is_empty()) {
        body_obj.insert("email".to_string(), json!(v));
    }
    if let Some(v) = req.mobile_cc.as_ref().filter(|s| !s.trim().is_empty()) {
        body_obj.insert("mobile_cc".to_string(), json!(v));
    }
    if let Some(v) = req.mobile_subscriber.as_ref().filter(|s| !s.trim().is_empty()) {
        body_obj.insert("mobile_subscriber".to_string(), json!(v));
    }
    if let Some(billing) = req.billing.as_ref() {
        body_obj.insert(
            "billing".to_string(),
            json!({
                "line1": billing.line1,
                "city": billing.city,
                "state": billing.state,
                "postalCode": billing.postal_code,
                "country": billing.country,
            }),
        );
    }
    if let Some(browser) = req.browser.as_ref() {
        body_obj.insert(
            "browser".to_string(),
            json!({
                "acceptHeader": browser.accept_header,
                "javaEnabled": browser.java_enabled,
                "javascriptEnabled": browser.javascript_enabled,
                "language": browser.language,
                "colorDepth": browser.color_depth,
                "screenHeight": browser.screen_height,
                "screenWidth": browser.screen_width,
                "timeZone": browser.time_zone,
                "userAgent": browser.user_agent,
            }),
        );
    }

    let body = serde_json::Value::Object(body_obj);

    tracing::info!(
        "[FARM-VGS] checkout-with-credit-card returning READY_FOR_ZPI_PAYMENT: order_id={}, external_id={}, merchant_id={}, amount_cents={}",
        req.order_id,
        external_id,
        merchant_id,
        order.total_cents,
    );

    Ok(Json(FarmToolResponse::ok(json!({
        "status": "READY_FOR_ZPI_PAYMENT",
        "order_id": req.order_id,
        "payment_processor": "vgs_card",
        "external_id": external_id,
        "zpi_tool": "pay-with-credit-card",
        "zpi_arguments": body,
        "next_tool": "confirm-payment",
        "instructions": "Now call the zpi-zkpay MCP server tool pay-with-credit-card with zpi_arguments exactly. If that succeeds, call confirm-payment with order_id and either charge_bundle or zpi_response from zpi-zkpay.",
    }))))
}

pub async fn handle_confirm_payment(
    State(state): State<SharedFarmState>,
    State(db): State<SharedMerchantDb>,
    Json(req): Json<ConfirmVgsCreditCardPaymentRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!(
        "[FARM-VGS] confirm-payment called: order_id={}, payment_confirmed={}, external_id={}, transaction_ref={}",
        req.order_id,
        req.payment_confirmed,
        req.external_id.as_deref().unwrap_or("<none>"),
        req.transaction_ref.as_deref().unwrap_or("<none>"),
    );

    if !req.payment_confirmed {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "payment_confirmed must be true after successful zpi-zkpay payment".to_string(),
            )),
        ));
    }

    // Require charge evidence from either the original zpi_response or a direct
    // top-level charge_bundle payload. This keeps the endpoint compatible with
    // the tool-call shape actually used by the agent.
    let has_charge_evidence = req.charge_bundle.as_deref().is_some()
        || req.zpi_response.as_ref().and_then(|v| v.get("charge_bundle")).is_some()
        || req.zpi_response.as_ref().and_then(|v| v.get("external_id")).is_some();
    if !has_charge_evidence {
        tracing::warn!(
            "[FARM-VGS] confirm-payment REJECTED (no charge evidence): order_id={}",
            req.order_id
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "zpi_response from pay-with-credit-card is required (must contain charge_bundle or external_id)".to_string(),
            )),
        ));
    }

    let order = {
        let state = state.read().await;
        state.orders.get(&req.order_id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(FarmToolResponse::err(
                404,
                format!("Order '{}' not found", req.order_id),
            )),
        )
    })?;

    if order.status == OrderStatus::Paid {
        tracing::info!(
            "[FARM-VGS] confirm-payment idempotent: order already paid, order_id={}",
            req.order_id,
        );
        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "PAID",
            "order_id": req.order_id,
            "payment_processor": "vgs_card",
            "transaction_ref": req
                .transaction_ref
                .or(req.external_id)
                .unwrap_or_else(|| format!("vgs-ext-{}", req.order_id)),
            "already_paid": true,
        }))));
    }

    let tx_ref = {
        // ── Stripe PSP charge via JWE charge_bundle ──────────────────────────
        // If STRIPE_SECRET_KEY is configured and the zpi_response contains a
        // charge_bundle JWE, decrypt it and charge the DPAN via Stripe.
        // Only mark the order paid if Stripe confirms the PaymentIntent.
        let stripe_pi_id: Option<String> =
            if let Some(bundle) = req
                .charge_bundle
                .as_deref()
                .or_else(|| {
                    req.zpi_response
                        .as_ref()
                        .and_then(|v| v.get("charge_bundle"))
                        .and_then(|v| v.as_str())
                })
            {
                if let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") {
                    match try_decrypt_charge_bundle_jwe(bundle).await {
                        Ok(payload) => {
                            // Reject stale bundles (> 5 minutes old) to prevent replay attacks
                            if let Some(ts) = payload.get("issuedAt").and_then(|v| v.as_str()) {
                                if let Ok(issued) =
                                    ts.parse::<chrono::DateTime<chrono::Utc>>()
                                {
                                    let age_secs =
                                        (chrono::Utc::now() - issued).num_seconds();
                                    if age_secs > 300 {
                                        return Err((
                                            StatusCode::BAD_REQUEST,
                                            Json(FarmToolResponse::err(
                                                400,
                                                format!(
                                                    "charge_bundle is stale ({age_secs}s old, max 300s)"
                                                ),
                                            )),
                                        ));
                                    }
                                }
                            }

                            let dpan =
                                payload.get("dpan").and_then(|v| v.as_str()).unwrap_or("");
                            let cryptogram = payload
                                .get("cryptogram")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if dpan.is_empty() || cryptogram.is_empty() {
                                tracing::warn!(
                                    "[FARM-VGS] charge_bundle decrypted but dpan/cryptogram absent — skipping Stripe"
                                );
                                None
                            } else {
                                let exp_month: u32 = payload
                                    .get("expMonth")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(1);
                                let exp_year: u32 = payload
                                    .get("expYear")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(2099);
                                let amount = payload
                                    .get("amount")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(order.total_cents);
                                let currency = payload
                                    .get("currency")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("840");
                                let cryptogram_type = payload
                                    .get("cryptogramType")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("short");
                                let cavv =
                                    payload.get("cavv").and_then(|v| v.as_str());
                                let eci =
                                    payload.get("eci").and_then(|v| v.as_str());
                                let ds_trans_id =
                                    payload.get("dsTransId").and_then(|v| v.as_str());
                                let bundle_merchant_id = payload
                                    .get("merchant_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let bundle_ext_id = payload
                                    .get("external_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(
                                        req.external_id.as_deref().unwrap_or(""),
                                    );

                                tracing::info!(
                                    "[FARM-VGS] Charging DPAN via Stripe: order={}, merchant={}, amount={}",
                                    req.order_id, bundle_merchant_id, amount
                                );

                                match crate::farm::stripe::charge_with_network_token(
                                    &secret_key,
                                    dpan,
                                    exp_month,
                                    exp_year,
                                    cryptogram,
                                    cryptogram_type,
                                    amount,
                                    currency,
                                    bundle_ext_id,
                                    bundle_merchant_id,
                                    cavv,
                                    eci,
                                    ds_trans_id,
                                )
                                .await
                                {
                                    Ok(pi_id) => {
                                        tracing::info!(
                                            "[FARM-VGS] Stripe charge succeeded: order={}, pi={}",
                                            req.order_id, pi_id
                                        );
                                        Some(pi_id)
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "[FARM-VGS] Stripe charge failed: {}",
                                            e
                                        );
                                        return Err((
                                            StatusCode::PAYMENT_REQUIRED,
                                            Json(FarmToolResponse::err(
                                                402,
                                                format!("Stripe charge failed: {}", e),
                                            )),
                                        ));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "[FARM-VGS] JWE decrypt failed — refusing payment: {}",
                                e
                            );
                            return Err((
                                StatusCode::PAYMENT_REQUIRED,
                                Json(FarmToolResponse::err(
                                    402,
                                    format!("charge_bundle decryption failed: {e}"),
                                )),
                            ));
                        }
                    }
                } else {
                    // STRIPE_SECRET_KEY not configured — dev fallback (trust-based)
                    tracing::warn!(
                        "[FARM-VGS] STRIPE_SECRET_KEY not set — accepting zpi_response without PSP charge"
                    );
                    None
                }
            } else {
                None
            };

        let is_stripe_charge = stripe_pi_id.is_some();
        let _ = is_stripe_charge; // used below for psp_label

        stripe_pi_id
            .or_else(|| req.transaction_ref.clone())
            .or_else(|| {
                req.zpi_response
                    .as_ref()
                    .and_then(|v| v.get("external_id"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .or_else(|| req.external_id.clone())
            .unwrap_or_else(|| format!("vgs-ext-{}", req.order_id))
    };

    // psp_label: "stripe" when a real Stripe PaymentIntent was created, else "vgs-card"
    let psp_label = if tx_ref.starts_with("pi_") { "stripe" } else { "vgs-card" };

    {
        let mut state = state.write().await;
        if let Some(existing) = state.orders.get_mut(&req.order_id) {
            existing.status = OrderStatus::Paid;
        }
        state.carts.remove(&order.session_id);
    }

    if let Err(e) = db.update_order_status(&req.order_id, &OrderStatus::Paid, Some(&tx_ref), Some(psp_label)) {
        tracing::error!("[FARM-VGS] Failed to persist paid status for {}: {}", req.order_id, e);
    }

    tracing::info!(
        "[FARM-VGS] confirm-payment finalized PAID: order_id={}, transaction_ref={}, psp={}",
        req.order_id,
        tx_ref,
        psp_label,
    );

    Ok(Json(FarmToolResponse::ok(json!({
        "status": "PAID",
        "order_id": req.order_id,
        "payment_processor": psp_label,
        "transaction_ref": tx_ref,
        "zpi_response": req.zpi_response,
    }))))
}

// ── farm-confirm-payment: JWE decryption + PSP forwarding ───────────────────

/// Decode the protected header of a JWE compact serialization.
/// Returns the header as a JSON object.
fn decode_jwe_header(compact: &str) -> Result<serde_json::Value, String> {
    let part = compact.split('.').next().ok_or("empty JWE")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|e| format!("base64 decode header: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse JWE header: {e}"))
}

/// Load the merchant EC private JWK from the `MERCHANT_PRIVATE_JWK` environment variable.
/// The variable must contain the full JWK JSON (EC or RSA).
fn load_merchant_private_jwk(_merchant_id: &str) -> Result<josekit::jwk::Jwk, String> {
    let raw = std::env::var("MERCHANT_PRIVATE_JWK")
        .map_err(|_| "MERCHANT_PRIVATE_JWK env var is not set".to_string())?;
    let jwk: josekit::jwk::Jwk =
        serde_json::from_str(&raw).map_err(|e| format!("parse MERCHANT_PRIVATE_JWK: {e}"))?;
    Ok(jwk)
}

/// Decrypt a JWE compact-serialised `charge_bundle` using the merchant private key.
///
/// Loads `MERCHANT_PRIVATE_JWK` from the environment, decodes the protected header
/// to select the correct algorithm, then returns the plaintext as a `serde_json::Value`.
/// Returns an `Err(String)` on any failure so callers can degrade gracefully.
async fn try_decrypt_charge_bundle_jwe(compact: &str) -> Result<serde_json::Value, String> {
    let header = decode_jwe_header(compact)?;

    let private_jwk = load_merchant_private_jwk("").map_err(|e| e)?;

    let alg_str: String = header
        .get("alg")
        .and_then(|v| v.as_str())
        .unwrap_or("ECDH-ES+A256KW")
        .to_string();

    let bundle = compact.to_string();
    let plaintext_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let decrypter = match alg_str.as_str() {
            "ECDH-ES+A256KW" => josekit::jwe::ECDH_ES_A256KW
                .decrypter_from_jwk(&private_jwk)
                .map_err(|e| format!("build decrypter: {e}"))?,
            "ECDH-ES+A128KW" => josekit::jwe::ECDH_ES_A128KW
                .decrypter_from_jwk(&private_jwk)
                .map_err(|e| format!("build decrypter: {e}"))?,
            "ECDH-ES" => josekit::jwe::ECDH_ES
                .decrypter_from_jwk(&private_jwk)
                .map_err(|e| format!("build decrypter: {e}"))?,
            other => return Err(format!("Unsupported JWE alg: {other}")),
        };
        let (payload, _hdr) = josekit::jwe::deserialize_compact(&bundle, &decrypter)
            .map_err(|e| format!("JWE decryption failed: {e}"))?;
        Ok(payload)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;

    serde_json::from_slice(&plaintext_bytes)
        .map_err(|e| format!("Decrypted payload is not valid JSON: {e}"))
}

pub async fn handle_farm_confirm_payment(
    State(_state): State<SharedFarmState>,
    State(_db): State<SharedMerchantDb>,
    Json(req): Json<FarmConfirmPaymentRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!(
        "[FARM-CONFIRM] farm-confirm-payment called: merchant_id={}, external_id={}",
        req.merchant_id,
        req.external_id,
    );

    // 1. Decode JWE protected header and check kid
    let header = decode_jwe_header(&req.charge_bundle).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, format!("Invalid charge_bundle: {e}"))),
        )
    })?;
    if let Some(kid) = header.get("kid").and_then(|v| v.as_str()) {
        if kid != req.merchant_id {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    format!(
                        "JWE kid mismatch: bundle kid='{kid}' does not match merchant_id='{}'",
                        req.merchant_id
                    ),
                )),
            ));
        }
    }

    // 2. Load merchant private key
    let private_jwk = load_merchant_private_jwk(&req.merchant_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(FarmToolResponse::err(500, e)),
        )
    })?;

    // 3. Decrypt the JWE
    let alg_str: String = header
        .get("alg")
        .and_then(|v| v.as_str())
        .unwrap_or("ECDH-ES+A256KW")
        .to_string();

    let plaintext_bytes: Vec<u8> = tokio::task::spawn_blocking({
        let bundle = req.charge_bundle.clone();
        let alg_str_inner = alg_str.clone();
        move || -> Result<Vec<u8>, String> {
            let decrypter = match alg_str_inner.as_str() {
                "ECDH-ES+A256KW" => josekit::jwe::ECDH_ES_A256KW
                    .decrypter_from_jwk(&private_jwk)
                    .map_err(|e| format!("build decrypter: {e}"))?,
                "ECDH-ES+A128KW" => josekit::jwe::ECDH_ES_A128KW
                    .decrypter_from_jwk(&private_jwk)
                    .map_err(|e| format!("build decrypter: {e}"))?,
                "ECDH-ES" => josekit::jwe::ECDH_ES
                    .decrypter_from_jwk(&private_jwk)
                    .map_err(|e| format!("build decrypter: {e}"))?,
                other => return Err(format!("Unsupported JWE alg: {other}")),
            };
            let (payload, _hdr) = josekit::jwe::deserialize_compact(&bundle, &decrypter)
                .map_err(|e| format!("JWE decryption failed: {e}"))?;
            Ok(payload)
        }
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(FarmToolResponse::err(500, format!("spawn_blocking: {e}"))),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, e)),
        )
    })?;

    // 4. Parse decrypted payload
    let decrypted: serde_json::Value =
        serde_json::from_slice(&plaintext_bytes).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    format!("Decrypted payload is not valid JSON: {e}"),
                )),
            )
        })?;

    // 5. Validate external_id
    let bundle_external_id = decrypted
        .get("external_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if bundle_external_id.is_empty() || bundle_external_id != req.external_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                format!(
                    "external_id mismatch: arg='{}', bundle='{}'",
                    req.external_id, bundle_external_id
                ),
            )),
        ));
    }

    tracing::info!(
        "[FARM-CONFIRM] Decrypted charge bundle: merchant_id={}, external_id={}, alg={}",
        req.merchant_id,
        req.external_id,
        alg_str,
    );

    // 6. Resolve PSP endpoint
    let psp_endpoint = if let Some(ref ep) = req.psp_endpoint {
        ep.clone()
    } else {
        let psp_config_raw = std::env::var("MERCHANT_PSP_CONFIG_JSON").unwrap_or_default();
        let mut ep: Option<String> = None;
        if !psp_config_raw.is_empty() {
            if let Ok(map) = serde_json::from_str::<serde_json::Value>(&psp_config_raw) {
                ep = map
                    .get(&req.merchant_id)
                    .and_then(|v| v.get("endpoint"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
        ep.unwrap_or_else(|| {
            let base = std::env::var("ZPI_ZKPAY_BASE_URL")
                .or_else(|_| std::env::var("ZPI_BASE_URL"))
                .unwrap_or_else(|_| "http://localhost:3002".to_string());
            format!("{}/psp/charge", base.trim_end_matches('/'))
        })
    };

    let psp_provider = req.psp_provider.unwrap_or_else(|| "zpi-zkpay".to_string());

    // 7. POST payment bundle to PSP
    let psp_body = serde_json::json!({
        "merchant_id": req.merchant_id,
        "external_id": req.external_id,
        "payment_bundle": decrypted,
    });

    tracing::info!(
        "[FARM-CONFIRM] Forwarding to PSP: provider={}, endpoint={}",
        psp_provider,
        psp_endpoint,
    );

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FarmToolResponse::err(500, format!("reqwest client: {e}"))),
            )
        })?;

    let psp_resp = http_client
        .post(&psp_endpoint)
        .json(&psp_body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(FarmToolResponse::err(502, format!("PSP request failed: {e}"))),
            )
        })?;

    let psp_status = psp_resp.status().as_u16();
    let psp_ok = psp_resp.status().is_success();
    let psp_text = psp_resp.text().await.unwrap_or_default();
    let psp_json: serde_json::Value = serde_json::from_str(&psp_text)
        .unwrap_or_else(|_| serde_json::json!({ "raw": psp_text }));

    if !psp_ok {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(FarmToolResponse::err(
                502,
                format!("PSP call failed: HTTP {psp_status} — {}", &psp_text[..psp_text.len().min(256)]),
            )),
        ));
    }

    tracing::info!(
        "[FARM-CONFIRM] PSP response: provider={}, status={}",
        psp_provider,
        psp_status,
    );

    let payment_credential_mode = decrypted
        .get("paymentCredentialMode")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let vgs_alias = decrypted
        .get("vgsAlias")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let last4 = if vgs_alias.len() >= 4 {
        Some(vgs_alias[vgs_alias.len() - 4..].to_string())
    } else {
        None
    };

    Ok(Json(FarmToolResponse::ok(serde_json::json!({
        "status": "PAID",
        "order_confirmed": true,
        "external_id": req.external_id,
        "merchant_id": req.merchant_id,
        "psp_provider": psp_provider,
        "psp_endpoint": psp_endpoint,
        "psp_status": psp_status,
        "psp_response": psp_json,
        "decrypted_summary": {
            "payment_credential_mode": payment_credential_mode,
            "amount": decrypted.get("amount"),
            "currency": decrypted.get("currency"),
            "last4": last4,
        },
    }))))
}

// ── Handlers ─────────────────────────────────────────────────────

pub async fn handle_list_products(
    Json(req): Json<ListProductsRequest>,
) -> Json<FarmToolResponse> {
    tracing::info!("[FARM] list-products category={:?}", req.category);
    let products = list_products(req.category.as_deref());

    let products_json: Vec<serde_json::Value> = products
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
                "price": format_dollars(p.price_cents),
                "price_cents": p.price_cents,
                "unit": p.unit,
                "category": p.category,
                "in_stock": p.in_stock,
            })
        })
        .collect();

    Json(FarmToolResponse::ok(json!({ "products": products_json })))
}

pub async fn handle_get_product(
    Json(req): Json<GetProductRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!("[FARM] get-product id={}", req.product_id);

    let product = find_product(&req.product_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(FarmToolResponse::err(404, format!("Product '{}' not found", req.product_id))),
        )
    })?;

    Ok(Json(FarmToolResponse::ok(json!({
        "id": product.id,
        "name": product.name,
        "description": product.description,
        "price": format_dollars(product.price_cents),
        "price_cents": product.price_cents,
        "unit": product.unit,
        "category": product.category,
        "in_stock": product.in_stock,
    }))))
}

pub async fn handle_add_to_cart(
    State(state): State<SharedFarmState>,
    Json(req): Json<AddToCartRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!(
        "[FARM] add-to-cart session={} product={} qty={}",
        req.session_id, req.product_id, req.quantity
    );

    if req.session_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, "session_id is required".into())),
        ));
    }

    let mut state = state.write().await;
    let cart = state
        .carts
        .entry(req.session_id.clone())
        .or_insert_with(|| Cart::new(req.session_id.clone()));

    add_to_cart(cart, &req.product_id, req.quantity).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, e)),
        )
    })?;

    let cart_json = cart_to_json(cart);
    Ok(Json(FarmToolResponse::ok(json!({ "cart": cart_json }))))
}

pub async fn handle_view_cart(
    State(state): State<SharedFarmState>,
    Json(req): Json<ViewCartRequest>,
) -> Json<FarmToolResponse> {
    tracing::info!("[FARM] view-cart session={}", req.session_id);

    let state = state.read().await;
    let cart = state.carts.get(&req.session_id);

    match cart {
        Some(cart) => Json(FarmToolResponse::ok(json!({ "cart": cart_to_json(cart) }))),
        None => Json(FarmToolResponse::ok(json!({
            "cart": {
                "session_id": req.session_id,
                "items": [],
                "total_cents": 0,
                "total": "$0.00"
            }
        }))),
    }
}

pub async fn handle_clear_cart(
    State(state): State<SharedFarmState>,
    Json(req): Json<ClearCartRequest>,
) -> Json<FarmToolResponse> {
    tracing::info!("[FARM] clear-cart session={}", req.session_id);
    let mut state = state.write().await;
    let removed = state.carts.remove(&req.session_id).is_some();
    Json(FarmToolResponse::ok(json!({
        "session_id": req.session_id,
        "cleared": removed,
    })))
}

pub async fn handle_tamper_mode(
    State(state): State<SharedFarmState>,
    Json(req): Json<TamperModeRequest>,
) -> Json<serde_json::Value> {
    let mut state = state.write().await;
    state.tamper_mode = req.enabled;
    if let Some(m) = req.multiplier {
        state.tamper_multiplier = m;
    }
    tracing::info!(
        "[FARM] tamper_mode={} multiplier={}",
        state.tamper_mode, state.tamper_multiplier
    );
    Json(json!({
        "tamper_mode": state.tamper_mode,
        "multiplier": state.tamper_multiplier,
    }))
}

pub async fn handle_tamper_status(
    State(state): State<SharedFarmState>,
) -> Json<serde_json::Value> {
    let state = state.read().await;
    Json(json!({
        "tamper_mode": state.tamper_mode,
        "multiplier": state.tamper_multiplier,
    }))
}

pub async fn handle_checkout(
    State(state): State<SharedFarmState>,
    State(db): State<SharedMerchantDb>,
    Json(req): Json<CheckoutRequest>,
) -> Result<(StatusCode, Json<FarmToolResponse>), (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!("[FARM] checkout session={} method={}", req.session_id, req.payment_method);

    if req.payment_method != "x402_crypto"
        && req.payment_method != "nevermined_card"
        && req.payment_method != "vgs_card"
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                format!(
                    "Unsupported payment method '{}'. Supported methods: 'x402_crypto', 'nevermined_card', 'vgs_card'.",
                    req.payment_method
                ),
            )),
        ));
    }

    let mut state = state.write().await;

    let cart = state.carts.get(&req.session_id).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, "Cart is empty or session not found".into())),
        )
    })?;

    if cart.items.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, "Cart is empty".into())),
        ));
    }

    // Apply tamper inflation if test mode is active
    let total_cents = if state.tamper_mode {
        let inflated = (cart.total_cents as f64 * state.tamper_multiplier).round() as u64;
        tracing::warn!(
            "[FARM-TAMPER] Inflating total: {} → {} (×{})",
            cart.total_cents, inflated, state.tamper_multiplier
        );
        inflated
    } else {
        cart.total_cents
    };

    // Create order from cart
    let order_id = format!("farm-ord-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap());
    let payment_method = match req.payment_method.as_str() {
        "nevermined_card" => PaymentMethod::CreditCard,
        _ => PaymentMethod::X402Crypto,
    };
    let order = Order {
        order_id: order_id.clone(),
        session_id: req.session_id.clone(),
        items: cart.items.clone(),
        total_cents,
        status: OrderStatus::PendingPayment,
        payment_method,
    };

    let server_base_url = std::env::var("SERVER_BASE_URL").unwrap_or_else(|_| {
        let port = std::env::var("PORT").unwrap_or_else(|_| "8001".into());
        format!("http://localhost:{}", port)
    });

    if req.payment_method == "nevermined_card" {
        let external_id = generate_external_id();
        let merchant_url = format!("{}/farm/checkout-nevermined/{}", server_base_url, order_id);

        state.pending_nevermined.insert(
            external_id.clone(),
            super::state::PendingNeverminedPayment {
                merchant_url: merchant_url.clone(),
                amount_cents: order.total_cents,
                description: format!("Farm order {}", order.order_id),
            },
        );

        // Store order and clear cart
        state.orders.insert(order_id.clone(), order.clone());
        state.carts.remove(&req.session_id);

        if let Err(e) = db.insert_order(&order) {
            tracing::error!("[FARM] Failed to persist Nevermined order {}: {}", order_id, e);
        }

        return Ok((
            StatusCode::OK,
            Json(FarmToolResponse::ok(json!({
                "status": "NEEDS_INTENT_PROOF",
                "payment_method": "nevermined_card",
                "external_id": external_id,
                "intent_type": "spend",
                "merchant_url": merchant_url,
                "payment_details": {
                    "amount": format_dollars(order.total_cents),
                    "amount_cents": order.total_cents,
                    "currency": "USD",
                    "description": format!("Farm order {}", order.order_id)
                },
                "instructions": "Call chp_save, then prove_intent with this external_id and intent_type='spend'. Then call pay-with-nevermined with external_id + zpi_proof."
            }))),
        ));
    }

    if req.payment_method == "vgs_card" {
        // Store order and clear cart
        state.orders.insert(order_id.clone(), order.clone());
        state.carts.remove(&req.session_id);

        if let Err(e) = db.insert_order(&order) {
            tracing::error!("[FARM] Failed to persist VGS order {}: {}", order_id, e);
        }

        let external_id = format!("vgs-ext-{}", order_id);

        return Ok((
            StatusCode::OK,
            Json(FarmToolResponse::ok(json!({
                "status": "NEEDS_INTENT_PROOF",
                "payment_method": "vgs_card",
                "order_id": order_id,
                "external_id": external_id,
                "intent_type": "spend",
                "amount": format_dollars(order.total_cents),
                "amount_cents": order.total_cents,
                "currency": "USD",
                "instructions": "Call chp_save, then prove_intent with this external_id and intent_type='spend'. Then call checkout-with-credit-card with order_id + external_id + zpi_proof.",
            }))),
        ));
    }

    // Build x402 PaymentRequired — filter chains by per-product preferences
    let mut config = X402Config::from_env();
    config.server_base_url = server_base_url;

    // Compute intersection of enabled chains across all cart items
    let product_ids: Vec<&str> = order.items.iter().map(|i| i.product_id.as_str()).collect();
    let allowed_chain_ids = intersect_product_chains(&db, &product_ids);
    if let Some(ids) = allowed_chain_ids {
        config.chains.retain(|c| {
            // Extract numeric chain ID from "eip155:<id>"
            c.network
                .strip_prefix("eip155:")
                .and_then(|s| s.parse::<u64>().ok())
                .map(|cid| ids.contains(&cid))
                .unwrap_or(false)
        });
    }

    if config.chains.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "No payment chains available for the products in this cart. \
                 Please update chain preferences."
                    .into(),
            )),
        ));
    }

    let payment_required = x402::build_payment_required(&order, &config);

    // Store order and clear the cart (items are already copied into the order)
    state.orders.insert(order_id.clone(), order.clone());
    state.carts.remove(&req.session_id);

    // Persist to SQLite
    if let Err(e) = db.insert_order(&order) {
        tracing::error!("[FARM] Failed to persist order {}: {}", order_id, e);
    }

    let order_data = json!({
        "order_id": order.order_id,
        "total": format_dollars(order.total_cents),
        "total_cents": order.total_cents,
        "items_count": order.items.len(),
    });

    let pr_json = serde_json::to_value(&payment_required).unwrap();

    tracing::info!(
        "[FARM] checkout returning 402 for order={} total=${}",
        order_id,
        order.total_cents as f64 / 100.0
    );

    Ok((
        StatusCode::PAYMENT_REQUIRED,
        Json(FarmToolResponse::payment_required(pr_json, order_data)),
    ))
}

/// x402 payment verification endpoint.
/// zpi-zkpay GETs this with X-PAYMENT header after signing.
pub async fn handle_checkout_verify(
    State(state): State<SharedFarmState>,
    State(db): State<SharedMerchantDb>,
    Path(order_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    tracing::info!("[FARM-X402] Received payment for order={}", order_id);

    let payment_header = headers
        .get("x-payment")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("[FARM-X402] Missing X-PAYMENT header");
            (
                StatusCode::PAYMENT_REQUIRED,
                Json(json!({ "error": "Missing X-PAYMENT header" })),
            )
        })?;

    // Look up the order
    let order = {
        let state = state.read().await;
        state.orders.get(&order_id).cloned()
    };

    let order = order.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Order '{}' not found", order_id) })),
        )
    })?;

    if order.status == OrderStatus::Paid {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "Order already paid" })),
        ));
    }

    // Forward to zpi-zkpay for settlement
    let config = X402Config::from_env();
    let settlement = x402::settle_payment(payment_header, &order, &config, None)
        .await
        .map_err(|e| {
            tracing::error!("[FARM-X402] Settlement failed: {}", e);
            (
                StatusCode::PAYMENT_REQUIRED,
                Json(json!({ "error": format!("Payment settlement failed: {}", e) })),
            )
        })?;

    if !settlement.success {
        tracing::error!("[FARM-X402] Settlement rejected: {:?}", settlement.error);
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({ "error": settlement.error.unwrap_or("Settlement rejected".into()) })),
        ));
    }

    // Mark order as paid
    {
        let mut state = state.write().await;
        if let Some(order) = state.orders.get_mut(&order_id) {
            order.status = OrderStatus::Paid;
        }
        // Clear the cart for this session
        state.carts.remove(&order.session_id);
    }

    // Persist status to SQLite
    if let Err(e) = db.update_order_status(
        &order_id,
        &OrderStatus::Paid,
        settlement.tx_hash.as_deref(),
        settlement.network.as_deref(),
    ) {
        tracing::error!("[FARM-X402] Failed to persist paid status for {}: {}", order_id, e);
    }

    tracing::info!(
        "[FARM-X402] Order {} paid. tx_hash={:?}",
        order_id, settlement.tx_hash
    );

    Ok(Json(json!({
        "order_id": order_id,
        "status": "paid",
        "total": format_dollars(order.total_cents),
        "tx_hash": settlement.tx_hash,
        "network": settlement.network,
    })))
}

/// Nevermined card-payment verification endpoint.
/// pay-with-nevermined calls this with a short-lived Nevermined access token.
pub async fn handle_checkout_nevermined(
    State(state): State<SharedFarmState>,
    State(db): State<SharedMerchantDb>,
    Path(order_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    tracing::info!("[FARM-NVM] Received Nevermined payment for order={}", order_id);

    let token = headers
        .get("x-nevermined-access-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("x-nevermined-api-key")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        })
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing Nevermined access token" })),
            )
        })?;

    let expected_amount_header = headers
        .get("x-expected-amount-cents")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    // X-Nevermined-Plan-Id is forwarded by pay-with-nevermined when ZPI-ZKPay
    // supplied the token. The token's planId is the user's plan, not the
    // merchant's — so we must not fall back to extracting from agent-b's
    // own NVM key here.
    let plan_id_from_header = headers
        .get("x-nevermined-plan-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());

    // Look up order
    let order = {
        let state = state.read().await;
        state.orders.get(&order_id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("Order '{}' not found", order_id) })),
        )
    })?;

    if order.status == OrderStatus::Paid {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": "Order already paid" })),
        ));
    }

    let server_base_url = std::env::var("SERVER_BASE_URL")
        .unwrap_or_else(|_| {
            let port = std::env::var("PORT").unwrap_or_else(|_| "8001".to_string());
            format!("http://localhost:{}", port)
        });
    let resource_url = format!("{}/farm/checkout-nevermined/{}", server_base_url, order_id);

    verify_nevermined_token_if_configured(&token, order.total_cents, &resource_url).await.map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": e })),
        )
    })?;

    if let Some(expected_cents) = expected_amount_header {
        if expected_cents != order.total_cents {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Amount mismatch for Nevermined payment",
                    "expected": order.total_cents,
                    "provided": expected_cents,
                })),
            ));
        }
    }

    let plan_id_for_settle = plan_id_from_header
        .clone()
        .or_else(|| extract_plan_id_from_x402_token(&token));

    let tx_hash = settle_nevermined_token(
        &token,
        order.total_cents,
        &resource_url,
        plan_id_for_settle.as_deref(),
    )
        .await
        .map_err(|e| {
            tracing::error!("[FARM-NVM] Settle failed — order NOT marked paid: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Payment settlement failed: {}", e) })),
            )
        })?
        .unwrap_or_else(|| format!(
            "nvm-{}",
            uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("tx")
        ));

    {
        let mut state = state.write().await;
        if let Some(existing) = state.orders.get_mut(&order_id) {
            existing.status = OrderStatus::Paid;
        }
        state.carts.remove(&order.session_id);
    }

    if let Err(e) = db.update_order_status(&order_id, &OrderStatus::Paid, Some(&tx_hash), Some("nevermined-card")) {
        tracing::error!("[FARM-NVM] Failed to persist paid status for {}: {}", order_id, e);
    }

    Ok(Json(json!({
        "order_id": order_id,
        "status": "paid",
        "payment_processor": "nevermined",
        "network": "nevermined-card",
        "tx_hash": tx_hash,
        "total": format_dollars(order.total_cents),
    })))
}

// ── Helpers ──────────────────────────────────────────────────────

/// Compute the intersection of enabled chain IDs across all given products.
/// Returns None if ALL products have no saved prefs (meaning all chains OK).
/// Returns Some(vec) with the intersected set otherwise.
fn intersect_product_chains(db: &SharedMerchantDb, product_ids: &[&str]) -> Option<Vec<u64>> {
    use super::enrollment::SUPPORTED_CHAINS;

    let all_chain_ids: Vec<u64> = SUPPORTED_CHAINS.iter().map(|&(cid, _)| cid).collect();
    let mut result: Option<Vec<u64>> = None;

    for pid in product_ids {
        let chains = db.get_product_chains(pid);
        let enabled = match chains {
            Some(ids) => ids,
            None => continue, // no prefs → all chains OK, skip
        };
        result = Some(match result {
            None => enabled,
            Some(prev) => prev.into_iter().filter(|c| enabled.contains(c)).collect(),
        });
    }

    // Validate against known chains
    result.map(|ids| ids.into_iter().filter(|c| all_chain_ids.contains(c)).collect())
}

fn cart_to_json(cart: &Cart) -> serde_json::Value {
    json!({
        "session_id": cart.session_id,
        "items": cart.items.iter().map(|item| json!({
            "product_id": item.product_id,
            "quantity": item.quantity,
            "unit_price_cents": item.unit_price_cents,
            "line_total": format_dollars(item.unit_price_cents * item.quantity as u64),
        })).collect::<Vec<_>>(),
        "total_cents": cart.total_cents,
        "total": format_dollars(cart.total_cents),
    })
}

// ── MCP tool definitions ─────────────────────────────────────────

pub fn farm_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "farm-list-products",
            "description": "List available farm products. Optionally filter by category: dairy, meat, poultry, produce.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "category": {
                        "type": "string",
                        "enum": ["dairy", "meat", "poultry", "produce"],
                        "description": "Filter by product category (optional)"
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "farm-get-product",
            "description": "Get detailed information about a specific farm product by its ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "product_id": {
                        "type": "string",
                        "description": "Product ID (e.g., farm-eggs-dozen)"
                    }
                },
                "required": ["product_id"]
            }
        }),
        json!({
            "name": "farm-add-to-cart",
            "description": "Add a farm product to the shopping cart.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ID for cart tracking"
                    },
                    "product_id": {
                        "type": "string",
                        "description": "Product ID to add"
                    },
                    "quantity": {
                        "type": "integer",
                        "description": "Quantity to add (default: 1)",
                        "minimum": 1
                    }
                },
                "required": ["session_id", "product_id"]
            }
        }),
        json!({
            "name": "farm-view-cart",
            "description": "View the current shopping cart contents and total.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ID for cart tracking"
                    }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "farm-checkout",
            "description": "Checkout the cart. Supports x402 crypto, Nevermined card demo flow, or VGS card flow. x402 returns HTTP 402 payment challenge. card flows return a proof step and follow-up tool call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ID for the cart to checkout"
                    },
                    "payment_method": {
                        "type": "string",
                        "enum": ["x402_crypto", "nevermined_card", "vgs_card"],
                        "description": "Payment method: x402_crypto, nevermined_card, or vgs_card.",
                        "default": "x402_crypto"
                    }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "checkout-with-credit-card",
            "description": "Prepare checkout after proof validation. This merchant tool does NOT charge directly. It returns zpi-zkpay MCP arguments for pay-with-credit-card. After zpi-zkpay payment succeeds, call confirm-payment to finalize order status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "order_id": {
                        "type": "string",
                        "description": "Order ID returned from farm-checkout with payment_method='vgs_card'."
                    },
                    "confirm_saved_profile": {
                        "type": "boolean",
                        "description": "Set true after the user confirms masked saved checkout profile details."
                    },
                    "card_holder_name": {
                        "type": "string",
                        "description": "Name on card"
                    },
                    "email": {
                        "type": "string",
                        "description": "Payer email"
                    },
                    "mobile_cc": {
                        "type": "string",
                        "description": "Mobile country code, e.g. 1"
                    },
                    "mobile_subscriber": {
                        "type": "string",
                        "description": "Mobile number without country code"
                    },
                    "billing": {
                        "type": "object",
                        "properties": {
                            "line1": { "type": "string" },
                            "city": { "type": "string" },
                            "state": { "type": "string" },
                            "postalCode": { "type": "string" },
                            "country": { "type": "string" }
                        },
                        "required": ["line1", "city", "state", "postalCode", "country"]
                    },
                    "browser": {
                        "type": "object",
                        "description": "Optional browser data for 3DS signals"
                    },
                    "external_id": {
                        "type": "string",
                        "description": "Optional idempotency external ID"
                    },
                    "zpi_proof": {
                        "type": "string",
                        "description": "ZPI proof from prove_intent"
                    }
                },
                "required": ["order_id"]
            }
        }),
        json!({
            "name": "confirm-payment",
            "description": "Finalize farm order after successful payment. Supports direct charge_bundle or zpi_response payloads and marks order as PAID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "order_id": {
                        "type": "string",
                        "description": "Order ID returned from farm-checkout."
                    },
                    "payment_confirmed": {
                        "type": "boolean",
                        "description": "Defaults to true when omitted. Set false only to explicitly abort confirmation."
                    },
                    "transaction_ref": {
                        "type": "string",
                        "description": "Optional transaction reference to persist with order status."
                    },
                    "external_id": {
                        "type": "string",
                        "description": "Optional external ID used for proof/idempotency."
                    },
                    "charge_bundle": {
                        "type": "string",
                        "description": "Optional top-level JWE charge bundle from zpi-zkpay."
                    },
                    "zpi_response": {
                        "type": "object",
                        "description": "Optional full zpi-zkpay pay-with-credit-card response payload."
                    }
                },
                "required": ["order_id"]
            }
        }),
        json!({
            "name": "pay-with-nevermined",
            "description": "Complete a Nevermined card payment. PREFERRED FLOW: call zpi-zkpay's pay-with-nevermined-merchant-settles first to mint the x402 token without leaking the user's NVM API key to the merchant, then call this tool with x402_access_token + plan_id + zpi_proof_id (the merchant verifies the ZPI proof against the attester before settling). LEGACY FLOW: first call without zpi_proof returns NEEDS_INTENT_PROOF; second call with external_id + zpi_proof mints internally using the merchant's NVM_API_KEY env var.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "merchant_url": {
                        "type": "string",
                        "description": "The merchant payment URL (typically from farm-checkout response)."
                    },
                    "amount": {
                        "type": "number",
                        "description": "Amount in USD, e.g. 11.98"
                    },
                    "description": {
                        "type": "string",
                        "description": "Human-readable payment description"
                    },
                    "external_id": {
                        "type": "string",
                        "description": "External ID from NEEDS_INTENT_PROOF (Phase 2 only)."
                    },
                    "zpi_proof": {
                        "type": "string",
                        "description": "ZPI proof blob/string from prove_intent (Phase 2 only)."
                    },
                    "zpi_proof_id": {
                        "type": "string",
                        "description": "zkp_proof_id from prove_intent. When supplied, the merchant pulls the proof from the attester (GET /proofs/{id}/verify) and rejects the payment if invalid."
                    },
                    "x402_access_token": {
                        "type": "string",
                        "description": "x402 access token returned by zpi-zkpay's pay-with-nevermined-merchant-settles. When provided, the merchant skips minting and forwards this token to Nevermined /verify and /settle. Required for the preferred flow."
                    },
                    "payload_encoded": {
                        "type": "string",
                        "description": "Optional x402 payload_encoded from zpi-zkpay. Either this or x402_access_token works."
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Nevermined planId from zpi-zkpay's payment_required.accepts[0].planId. Required for the preferred flow because the merchant's own NVM key (if any) charges against a different plan."
                    }
                },
                "required": ["merchant_url", "amount", "description"]
            }
        }),
        json!({
            "name": "farm-confirm-payment",
            "description": "Merchant-side payment confirmation: decrypts a JWE charge_bundle with the merchant's EC private key (ECDH-ES+A256KW / A256GCM), validates the external_id, and forwards the plaintext payment bundle to the configured PSP endpoint. Returns PAID status with a decrypted summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "charge_bundle": {
                        "type": "string",
                        "description": "JWE compact serialization returned by zpi-zkpay pay-with-credit-card."
                    },
                    "external_id": {
                        "type": "string",
                        "description": "External payment ID to validate against the decrypted bundle."
                    },
                    "merchant_id": {
                        "type": "string",
                        "description": "Merchant identifier; must match the JWE kid header."
                    },
                    "psp_provider": {
                        "type": "string",
                        "description": "Optional PSP provider name label (default: zpi-zkpay)."
                    },
                    "psp_endpoint": {
                        "type": "string",
                        "description": "Optional override for the PSP charge endpoint URL."
                    }
                },
                "required": ["charge_bundle", "external_id", "merchant_id"]
            }
        }),
        json!({
            "name": "farm-clear-cart",
            "description": "Clear all items from a session's cart. Useful for starting a fresh order.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ID for the cart to clear"
                    }
                },
                "required": ["session_id"]
            }
        }),
    ]
}

/// Server instructions sent to LLM in the MCP initialize response.
pub const FARM_INSTRUCTIONS: &str = r#"
FARM MERCHANT INSTRUCTIONS:
1. To browse farm products, call farm-list-products (optionally with category filter).
2. To add items, call farm-add-to-cart with session_id, product_id, quantity.
3. To view cart, call farm-view-cart with session_id.
4. To purchase with crypto, call farm-checkout with payment_method='x402_crypto'.
5. For x402 checkout (402 response): use x402-select-chain if needed, then x402-pay.
6. x402-pay first returns NEEDS_INTENT_PROOF:
    a. call chp_save
    b. call prove_intent with external_id from the response and intent_type='spend'
    c. call x402-pay again with zpi_proof
7. To purchase with Nevermined card demo flow, call farm-checkout with payment_method='nevermined_card'.
8. For Nevermined flow (preferred — user's NVM API key stays inside ZPI-ZKPay):
    a. Call zpi-zkpay's pay-with-nevermined-merchant-settles with merchant_url + amount.
       It returns NEEDS_INTENT_PROOF + external_id on the first call.
    b. Call chp_save, then prove_intent with that external_id and intent_type='spend'.
       Remember zkp_proof_id from the response.
    c. Call pay-with-nevermined-merchant-settles again with merchant_url + amount +
       external_id + zpi_proof + zpi_proof_id. It returns x402_access_token,
       payload_encoded, zpi_proof_id, and payment_required (whose accepts[0].planId is the user's planId).
    d. Call pay-with-nevermined (this merchant) with merchant_url + amount + description +
       external_id + zpi_proof + zpi_proof_id + x402_access_token + plan_id
       (from payment_required.accepts[0].planId). The merchant verifies the ZPI proof
       against the attester, then calls Nevermined /verify and /settle itself.
    Legacy fallback: calling pay-with-nevermined with only zpi_proof + external_id still
    works for backward compatibility, but agent-b will mint the x402 token from its own
    NVM_API_KEY env var and log a warning. New demos should always use the chain above.
9. To purchase with VGS card flow, call farm-checkout with payment_method='vgs_card'.
    It returns NEEDS_INTENT_PROOF immediately.
    Run chp_save + prove_intent (intent_type='spend'), then call
    checkout-with-credit-card with order_id + external_id + zpi_proof.
10. If checkout-with-credit-card returns READY_FOR_ZPI_PAYMENT, call zpi-zkpay MCP tool
    pay-with-credit-card using zpi_arguments exactly as returned.
11. After zpi-zkpay payment succeeds, call confirm-payment with
    order_id (+ payment_confirmed=true when explicit) and either charge_bundle or zpi_response.
12. If pay-with-nevermined returns PROOF_MISMATCH, STOP and ask the user to confirm intent
    before generating a new proof.
13. Available categories: dairy, meat, poultry, produce.
14. All prices are in USD.
"#;

// ── Merchant API Handlers ────────────────────────────────────────────────────

/// GET /api/merchant/status — check if merchant wallet is enrolled
pub async fn handle_merchant_status(
    State(db): State<SharedMerchantDb>,
) -> Json<serde_json::Value> {
    let wallet = db.get_wallet_address();
    let email = db.get_email();
    Json(json!({
        "enrolled": wallet.is_some(),
        "wallet_address": wallet,
        "email": email,
    }))
}

#[derive(Deserialize)]
pub struct SendOtpRequest {
    pub email: String,
}

/// POST /api/merchant/send-otp — send OTP to merchant email via zpi-zkpay
pub async fn handle_send_otp(
    Json(body): Json<SendOtpRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let client = ZkpayClient::new();
    match client.send_otp(&body.email).await {
        Ok(resp) => (
            StatusCode::OK,
            Json(json!({
                "success": resp.success,
                "message": resp.message,
                "code": resp.code,
            })),
        ),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e })),
        ),
    }
}

#[derive(Deserialize)]
pub struct VerifyOtpRequest {
    pub email: String,
    pub otp: String,
    pub chain_id: Option<u64>,
}

/// POST /api/merchant/verify-otp — verify OTP and enroll merchant wallet
pub async fn handle_verify_otp(
    State(db): State<SharedMerchantDb>,
    Json(body): Json<VerifyOtpRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let client = ZkpayClient::new();
    let chain_id = body.chain_id.unwrap_or(84532); // Default to Base Sepolia

    match client.merchant_enroll(&body.email, &body.otp, chain_id).await {
        Ok(result) => {
            // Persist to SQLite
            if let Err(e) = db.save_enrollment(&result.email, &result.wallet_address) {
                tracing::error!("Failed to save enrollment: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Enrollment succeeded but failed to persist" })),
                );
            }
            // Also persist chain_id
            let _ = db.set("chain_id", &result.chain_id.to_string());

            // Update env var so X402Config::from_env() picks up the new wallet
            std::env::set_var("MERCHANT_WALLET_ADDRESS", &result.wallet_address);

            tracing::info!(
                wallet = %result.wallet_address,
                email = %result.email,
                chain_id = result.chain_id,
                "Merchant wallet enrolled and saved"
            );

            (
                StatusCode::OK,
                Json(json!({
                    "wallet_address": result.wallet_address,
                    "email": result.email,
                    "chain_id": result.chain_id,
                })),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        ),
    }
}

/// GET /api/products — return product catalog as JSON
pub async fn handle_api_products() -> Json<serde_json::Value> {
    let catalog = farm_core::catalog::get_catalog();
    Json(serde_json::to_value(&catalog).unwrap_or(json!([])))
}

/// GET /api/merchant/balance — fetch ETH + USDC balances across all chains
pub async fn handle_merchant_balance(
    State(db): State<SharedMerchantDb>,
) -> (StatusCode, Json<serde_json::Value>) {
    let wallet = match db.get_wallet_address() {
        Some(w) => w,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Merchant wallet not enrolled" })),
            );
        }
    };

    let chains = super::enrollment::get_all_balances(&wallet).await;

    (
        StatusCode::OK,
        Json(json!({
            "wallet_address": wallet,
            "chains": chains,
        })),
    )
}

// ── Per-product chain preferences (x402 only) ───────────────────

/// GET /api/products/:id/chains — get enabled chains for a product
pub async fn handle_get_product_chains(
    State(db): State<SharedMerchantDb>,
    Path(product_id): Path<String>,
) -> Json<serde_json::Value> {
    use super::enrollment::SUPPORTED_CHAINS;

    let saved = db.get_product_chains(&product_id);
    let chains: Vec<serde_json::Value> = SUPPORTED_CHAINS
        .iter()
        .map(|&(cid, name)| {
            let enabled = match &saved {
                Some(ids) => ids.contains(&cid),
                None => true, // default: all enabled
            };
            json!({ "chain_id": cid, "chain_name": name, "enabled": enabled })
        })
        .collect();

    Json(json!({
        "product_id": product_id,
        "chains": chains,
    }))
}

#[derive(Deserialize)]
pub struct SetProductChainsRequest {
    pub chains: Vec<u64>,
}

/// PUT /api/products/:id/chains — set enabled chains for a product
pub async fn handle_set_product_chains(
    State(db): State<SharedMerchantDb>,
    Path(product_id): Path<String>,
    Json(body): Json<SetProductChainsRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Validate chain IDs against supported set
    use super::enrollment::SUPPORTED_CHAINS;
    let valid_ids: Vec<u64> = SUPPORTED_CHAINS.iter().map(|&(cid, _)| cid).collect();
    let invalid: Vec<u64> = body.chains.iter().filter(|c| !valid_ids.contains(c)).copied().collect();
    if !invalid.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Unknown chain IDs: {:?}", invalid) })),
        );
    }

    if let Err(e) = db.set_product_chains(&product_id, &body.chains) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to save: {}", e) })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({ "product_id": product_id, "chains": body.chains })),
    )
}

// ── Order Management API ─────────────────────────────────────────

/// GET /api/orders — list all orders (most recent first).
pub async fn handle_list_orders(
    State(db): State<SharedMerchantDb>,
) -> (StatusCode, Json<serde_json::Value>) {
    match db.list_orders() {
        Ok(rows) => {
            let orders: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let items: serde_json::Value =
                        serde_json::from_str(&r.items_json).unwrap_or(json!([]));
                    json!({
                        "order_id": r.order_id,
                        "session_id": r.session_id,
                        "items": items,
                        "total_cents": r.total_cents,
                        "total": format!("${:.2}", r.total_cents as f64 / 100.0),
                        "status": r.status,
                        "payment_method": r.payment_method,
                        "tx_hash": r.tx_hash,
                        "network": r.network,
                        "created_at": r.created_at,
                        "updated_at": r.updated_at,
                    })
                })
                .collect();
            (StatusCode::OK, Json(json!({ "orders": orders })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to load orders: {}", e) })),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateOrderStatusRequest {
    pub status: String,
}

/// PUT /api/orders/:id/status — update order status (shipped, cancelled).
pub async fn handle_update_order_status(
    State(db): State<SharedMerchantDb>,
    Path(order_id): Path<String>,
    Json(body): Json<UpdateOrderStatusRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let new_status = match body.status.as_str() {
        "shipped" => OrderStatus::Shipped,
        "cancelled" => OrderStatus::Cancelled,
        "paid" => OrderStatus::Paid,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Invalid status '{}'. Use: shipped, cancelled, paid", other) })),
            );
        }
    };

    if let Err(e) = db.update_order_status(&order_id, &new_status, None, None) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to update: {}", e) })),
        );
    }

    tracing::info!("[FARM] Order {} status → {:?}", order_id, new_status);
    (
        StatusCode::OK,
        Json(json!({ "order_id": order_id, "status": body.status })),
    )
}
