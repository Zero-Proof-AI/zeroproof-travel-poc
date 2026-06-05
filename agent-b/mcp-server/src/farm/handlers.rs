use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

use farm_core::cart::add_to_cart;
use farm_core::catalog::{find_product, list_products};
use farm_core::types::{Cart, Order, OrderStatus, PaymentMethod};

use super::db::SharedMerchantDb;
use super::enrollment::ZkpayClient;
use super::state::SharedFarmState;
use super::x402::{self, X402Config};
use url::Url;

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
    #[serde(default)]
    pub proof_id: Option<String>,
    #[serde(default)]
    pub x402_access_token: Option<String>,
    #[serde(default)]
    pub payload_encoded: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub token_ref: Option<String>,
    #[serde(default)]
    pub verify_only: Option<bool>,
    #[serde(default)]
    pub currency: Option<String>,
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
    pub payment_confirmed: bool,
    #[serde(default)]
    pub transaction_ref: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub zpi_response: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct FarmConfirmPaymentRequest {
    pub charge_bundle: String,
    pub external_id: String,
    /// Merchant identifier; must match the JWE `kid` header. Optional for the
    /// lean Nevermined flow — when omitted it is resolved from the bundle's
    /// `kid` (and falls back to `ZPI_VGS_MERCHANT_ID`).
    #[serde(default)]
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

    pub fn err_with_data(status: u16, error: String, data: serde_json::Value) -> Self {
        Self {
            success: false,
            data: Some(data),
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

fn generate_external_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn amount_to_cents(amount: f64) -> Result<u64, String> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err("amount must be a positive number".into());
    }
    Ok((amount * 100.0).round() as u64)
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
                if lower.contains("token")
                    || lower.contains("authorization")
                    || lower.contains("api_key")
                    || lower.contains("apikey")
                    || lower.contains("secret")
                    || lower.contains("encoded")
                    || lower.contains("payload")
                    || lower == "proof"
                    || lower.contains("raw_proof")
                    || lower.contains("code")
                {
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

fn redact_body_text(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .map(|v| redact_json_value(&v).to_string())
        .unwrap_or_else(|_| {
            if body.len() > 120 {
                let preview: String = body.chars().take(120).collect();
                format!("{}…<redacted>", preview)
            } else {
                "<redacted>".to_string()
            }
        })
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
                    tracing::info!(
                        "[FARM-NVM][HTTP][RESPONSE] method=POST url={} attempt={}/{} status={} body={}",
                        token_url,
                        attempt,
                        max_attempts,
                        status,
                        redact_body_text(&raw_body)
                    );
                }

                if status.is_success() {
                    let body =
                        serde_json::from_str::<serde_json::Value>(&raw_body).map_err(|e| {
                            format!(
                                "failed to parse Nevermined token response: {}; raw={}",
                                e,
                                redact_body_text(&raw_body)
                            )
                        })?;

                    // NVM Pay responses vary by endpoint; accept common token field names.
                    // v2 x402 shape returns payloadEncoded (base64) — prefer that.
                    let token_val = body
                        .get("payloadEncoded")
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
                            redact_json_value(&body)
                        )
                    });
                }

                let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
                last_error = Some(format!(
                    "Nevermined token exchange failed: status={} body={}",
                    status,
                    redact_body_text(&raw_body)
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


async fn verify_nevermined_token_if_configured(
    token: &str,
    amount_cents: u64,
    resource_url: &str,
    plan_id_override: Option<&str>,
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
                .unwrap_or(false),
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
                verify_url, status, redact_body_text(&raw_body)
            );
        }
        if status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
            tracing::info!("Nevermined Visa verify response: {}", redact_json_value(&body));
            return Ok(());
        }
        return Err(format!("Nevermined Visa verify {} ({})", status, redact_body_text(&raw_body)));
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
        let scheme =
            std::env::var("NEVERMINED_SCHEME").unwrap_or_else(|_| "nvm:erc4337".to_string());
        let network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| {
            if scheme == "nvm:card-delegation" {
                "stripe".to_string()
            } else {
                "eip155:84532".to_string()
            }
        });
        let plan_id = plan_id_override
            .map(|s| s.to_string())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| extract_plan_id_from_x402_token(token))
            .or_else(|| {
                std::env::var("NEVERMINED_PLAN_ID")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .or_else(|| {
                std::env::var("NVM_API_KEY")
                    .ok()
                    .and_then(|k| extract_nvm_plan_id(&k))
            })
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

    let resp = req.json(&verify_body).send().await.map_err(|e| {
        format!(
            "failed to verify Nevermined credential at {}: {}",
            verify_url, e
        )
    })?;

    let status = resp.status();
    let raw_body = resp.text().await.unwrap_or_default();

    if http_trace {
        tracing::info!(
            "[FARM-NVM][HTTP][RESPONSE] method=POST url={} status={} body={}",
            verify_url,
            status,
            redact_body_text(&raw_body)
        );
    }

    if status.is_success() {
        let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
        tracing::info!("Nevermined verify response: {}", redact_json_value(&body));
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
        let scheme =
            std::env::var("NEVERMINED_SCHEME").unwrap_or_else(|_| "nvm:erc4337".to_string());
        let network = std::env::var("NEVERMINED_NETWORK").unwrap_or_else(|_| {
            if scheme == "nvm:card-delegation" {
                "stripe".to_string()
            } else {
                "eip155:84532".to_string()
            }
        });
        let plan_id = plan_id_override
            .map(|s| s.to_string())
            .filter(|v| !v.trim().is_empty())
            .or_else(|| extract_plan_id_from_x402_token(token))
            .or_else(|| {
                std::env::var("NEVERMINED_PLAN_ID")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .or_else(|| {
                std::env::var("NVM_API_KEY")
                    .ok()
                    .and_then(|k| extract_nvm_plan_id(&k))
            })
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
            tracing::info!(
                "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=legacy-fallback status={} body={}",
                verify_url,
                fallback_status,
                redact_body_text(&fallback_raw_body)
            );
        }

        if fallback_status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&fallback_raw_body).unwrap_or_default();
            tracing::info!("Nevermined verify response (legacy fallback): {}", redact_json_value(&body));
            return Ok(());
        }

        return Err(format!(
            "Nevermined verify failed api={} ({}) legacy-fallback={} ({})",
            status,
            redact_body_text(&raw_body),
            fallback_status,
            redact_body_text(&fallback_raw_body)
        ));
    }

    Err(format!("Nevermined facilitator {} ({})", status, redact_body_text(&raw_body)))
}

async fn settle_nevermined_token(
    token: &str,
    _amount_cents: u64,
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
                settle_url, status, redact_body_text(&raw_body)
            );
        }
        if status.is_success() {
            let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
            tracing::info!("Nevermined Visa settle response: {}", redact_json_value(&body));
            let tx = body.get("transaction").and_then(|v| v.as_str()).map(|s| s.to_string());
            return Ok(tx);
        }
        return Err(format!("Nevermined Visa settle {} ({})", status, redact_body_text(&raw_body)));
    }

    // Nevermined credits are NOT cents. `creditsUsed` is the number of plan
    // credits to burn per request (typically 1). Passing `amount_cents` here
    // would request burning thousands of credits for a single checkout.
    // Allow override via NEVERMINED_CREDITS_PER_REQUEST; default to 1.
    let credits_used: i64 = std::env::var("NEVERMINED_CREDITS_PER_REQUEST")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(1);

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
            std::env::var("NEVERMINED_PLAN_ID")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("NVM_API_KEY")
                .ok()
                .and_then(|k| extract_nvm_plan_id(&k))
        })
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
        tracing::info!(
            "[FARM-NVM][HTTP][RESPONSE] method=POST url={} schema=facilitator status={} body={}",
            settle_url,
            status,
            redact_body_text(&raw_body)
        );
    }

    if status.is_success() {
        let body = serde_json::from_str::<serde_json::Value>(&raw_body).unwrap_or_default();
        tracing::info!("Nevermined settle response: {}", redact_json_value(&body));
        let tx_hash = body.get("txHash").and_then(|v| v.as_str()).map(|s| s.to_string());
        Ok(tx_hash)
    } else {
        Err(format!("Nevermined settle {} ({})", status, redact_body_text(&raw_body)))
    }
}

/// Default attester base URL when `ZPI_ATTESTER_URL` is unset.
fn default_zpi_attester_url() -> String {
    std::env::var("ZPI_ATTESTER_URL").unwrap_or_else(|_| "http://localhost:8000".to_string())
}

/// Whether proofs must commit a `total_amount` field (strict by default).
///
/// Unset → `true`. Only an explicit opt-out disables the check:
/// `ZPI_REQUIRE_AMOUNT_COMMITMENT=false` or `=0`.
fn zpi_require_amount_commitment() -> bool {
    match std::env::var("ZPI_REQUIRE_AMOUNT_COMMITMENT") {
        Ok(v) => {
            let v = v.trim();
            !(v.eq_ignore_ascii_case("false") || v == "0")
        }
        Err(_) => true,
    }
}

/// Whether the merchant re-verifies the ZPI intent proof against the attester
/// before settling (optional per Nevermined design — the merchant may either
/// re-verify, or trust ZPI-ZKPay's pre-mint proof gate).
///
/// Default → `true` (re-verify, the safe default). This applies in BOTH the
/// intent and proof verification modes: when enabled, the merchant runs its
/// own zero-trust re-check; when disabled it skips straight to settlement and
/// trusts the zkpay proof gate + the JWE trust boundary.
///
/// Disable with `ZPI_MERCHANT_VERIFY=false`, `=0`, `=off`, or `=no`.
fn zpi_merchant_verify_enabled() -> bool {
    match std::env::var("ZPI_MERCHANT_VERIFY") {
        Ok(v) => {
            let v = v.trim();
            !(v.eq_ignore_ascii_case("false")
                || v == "0"
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("no"))
        }
        Err(_) => true,
    }
}

/// Canonical ISO 4217 currency for merchant ZPI binding (trim + uppercase).
/// Missing or empty defaults to `USD` for backward compatibility.
fn canonicalize_merchant_currency(currency: Option<&str>) -> String {
    currency
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("USD")
        .to_ascii_uppercase()
}

/// Actionable JSON error for proof field binding failures (parsed by agents/tests).
fn proof_binding_error(field: &str, merchant_canonical: &str, hint: &str, message: &str) -> String {
    serde_json::to_string(&json!({
        "status": "PROOF_BINDING_FAILED",
        "field": field,
        "merchant_canonical": merchant_canonical,
        "hint": hint,
        "message": message,
    }))
    .unwrap_or_else(|_| {
        format!(
            "[FARM-NVM][ZPI] {field} binding FAILED — {message} (merchant canonical: {merchant_canonical}; hint: {hint})"
        )
    })
}

fn proof_verification_error_response(status: u16, error: String) -> FarmToolResponse {
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&error) {
        let message = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("proof verification failed")
            .to_string();
        FarmToolResponse::err_with_data(status, message, data)
    } else {
        FarmToolResponse::err(status, error)
    }
}

fn proof_id_from_request(req: &PayWithNeverminedRequest) -> Option<&str> {
    req.proof_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Verify a ZPI proof against the zk-attestation-service (zero-trust).
///
/// Before settling, the merchant pulls the proof from the attester and
/// verifies it *against its own record* rather than trusting an opaque
/// `verified` flag. Without this, any blob Claude forwards as "proof" — or a
/// valid proof for a *different* transaction — would pass the gate.
///
/// Behaviour:
///   * If `proof_id` is `None` or empty → returns Ok (soft-skip). We
///     warn so the missing proof shows up in the log, but we don't block —
///     existing demos that haven't been updated to pass `proof_id` keep
///     working.
///   * Otherwise, two steps:
///     1. **Liveness** — `GET /proofs/{id}/verify` confirms the proof exists
///        and is fresh (≤ 5 min). If a supplied legacy id 404s, we resolve the
///        attester's real UUID via `GET /proofs/session/{external_id}` and retry.
///     2. **Zero-trust binding** — `GET /proofs/{id}` returns the raw record;
///        we parse the SP1-committed `public_values` and independently assert
///        the committed `external_id` is *this* transaction and the committed
///        `total_amount` field commitment matches the amount we're charging,
///        re-derived from our own cart with the per-tx salt. The merchant
///        trusts neither Claude nor the attester's flat metadata fields.
///
/// Note: this does not cryptographically verify the Groth16 proof bytes
/// (that needs the SP1 verifier + program ELF embedded in the merchant);
/// it verifies the proof's public-input bindings against the merchant record.
async fn verify_zpi_proof_against_attester(
    proof_id: Option<&str>,
    external_id: &str,
    expected_amount_cents: u64,
    expected_currency: &str,
    expected_merchant_url: &str,
) -> Result<String, String> {
    let attester_url = default_zpi_attester_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client for attester: {}", e))?;

    // Resolve the proof id to re-check. The lean VGS-style flow does not thread
    // a proof_id through the charge_bundle, so when it is absent we resolve the
    // proof directly from the attester by external_id (the attester keys proofs
    // by session = external_id). In proof-mode a proof exists and we still run
    // the full zero-trust re-check; in intent-mode (no proof on file) we
    // soft-skip and trust the zkpay proof gate + the JWE trust boundary.
    let supplied_id: String = match proof_id.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => match resolve_attester_proof_id_by_external_id(&client, &attester_url, external_id)
            .await
        {
            Ok(resolved) => {
                tracing::info!(
                    "[FARM-NVM][ZPI] No proof_id in charge_bundle; resolved proof_id={} from external_id={} for merchant re-check",
                    resolved,
                    external_id
                );
                resolved
            }
            Err(e) => {
                tracing::warn!(
                    "[FARM-NVM][ZPI] No proof_id in charge_bundle and no attester proof for external_id={} amount_cents={} merchant_url={} ({}). Soft-skipping merchant re-check — trusting the zkpay proof gate + JWE boundary.",
                    external_id,
                    expected_amount_cents,
                    expected_merchant_url,
                    e
                );
                return Ok(String::new());
            }
        },
    };

    // Step 1 — confirm the proof exists and is fresh (attester `/verify`
    // does a 5-minute freshness check + `verified` flag). This is a liveness
    // gate only; the authoritative checks happen in step 2 against the
    // committed public values. We still resolve through the existing
    // legacy-ID → attester-UUID fallback so older flows work.
    let verified_proof_id = match verify_attester_proof_by_id(&client, &attester_url, &supplied_id)
        .await
    {
        AttesterVerifyOutcome::Ok { session_id } => {
            tracing::info!(
                "[FARM-NVM][ZPI] attester confirms proof exists + fresh proof_id={} session_id={} external_id={}",
                supplied_id,
                session_id.as_deref().unwrap_or("<none>"),
                external_id
            );
            supplied_id.to_string()
        }
        AttesterVerifyOutcome::NotFound => {
            tracing::warn!(
                "[FARM-NVM][ZPI] supplied proof_id={} not recognised by the attester — \
                 falling back to lookup by external_id={}. This usually means the zpi-cli \
                 returned a legacy local id rather than the attester's UUID.",
                supplied_id,
                external_id
            );
            let resolved =
                resolve_attester_proof_id_by_external_id(&client, &attester_url, external_id)
                    .await?;
            match verify_attester_proof_by_id(&client, &attester_url, &resolved).await {
                AttesterVerifyOutcome::Ok { session_id } => {
                    tracing::info!(
                        "[FARM-NVM][ZPI] attester confirms proof exists + fresh (via external_id fallback) proof_id={} session_id={} external_id={} supplied_id={}",
                        resolved,
                        session_id.as_deref().unwrap_or("<none>"),
                        external_id,
                        supplied_id
                    );
                    resolved
                }
                AttesterVerifyOutcome::NotFound => {
                    return Err(format!(
                        "[FARM-NVM][ZPI] resolved proof_id={} (from external_id={}) was not found by the attester after lookup",
                        resolved, external_id
                    ));
                }
                AttesterVerifyOutcome::Failed(msg) => return Err(msg),
            }
        }
        AttesterVerifyOutcome::Failed(msg) => return Err(msg),
    };

    // Step 2 — ZERO-TRUST verification against the committed public values.
    // Instead of trusting the attester's `verified` flag, we pull the raw
    // proof record, parse the SP1 `public_values` the zkVM guest committed,
    // and independently re-derive the bindings from the merchant's OWN record:
    //   * the committed `external_id` MUST equal the transaction we're settling
    //     (anti-substitution / anti-replay), and
    //   * the committed `total_amount` field commitment MUST equal the amount
    //     we're about to charge, re-hashed from our cart with the per-tx salt
    //     (a malicious payer cannot inflate the amount or reuse another proof).
    let material = fetch_proof_material_by_id(&client, &attester_url, &verified_proof_id)
        .await?
        .ok_or_else(|| {
            format!(
                "[FARM-NVM][ZPI] proof material disappeared for proof_id={} (external_id={}) between verify and fetch",
                verified_proof_id, external_id
            )
        })?;

    run_zero_trust_proof_checks(
        &client,
        &attester_url,
        &material,
        external_id,
        expected_amount_cents,
        expected_currency,
    )
    .await?;

    verify_sp1_proof_with_attester(
        &client,
        &attester_url,
        &verified_proof_id,
        external_id,
        expected_amount_cents,
        expected_currency,
        material.program_id.as_deref(),
        material.vk_hash.as_deref(),
    )
    .await?;

    tracing::info!(
        "[FARM-NVM][ZPI] ✅ zero-trust verification passed proof_id={} external_id={} amount_cents={} currency={} merchant_url={} — proof binds to this transaction + amount + currency, accepting payment",
        verified_proof_id,
        external_id,
        expected_amount_cents,
        expected_currency,
        expected_merchant_url
    );
    Ok(verified_proof_id)
}

/// Raw proof material pulled from attester proof records (`data.proof.proof`).
///
/// `public_values_hex` is the authoritative, zkVM-committed byte string — the
/// merchant parses it directly rather than trusting the attester's flat
/// metadata fields. `field_commitments_meta` is the attester's *separate* JSON
/// copy, used only as a defence-in-depth cross-check.
struct ProofMaterial {
    public_values_hex: String,
    vk_hash: Option<String>,
    program_id: Option<String>,
    field_commitments_meta: Option<serde_json::Value>,
}

/// Parse proof verification material from a `SingleProofResponse`-shaped JSON
/// body (`GET /proofs/{id}`).
fn extract_proof_material_from_json(parsed: &serde_json::Value) -> Option<ProofMaterial> {
    let success = parsed.get("success").and_then(|v| v.as_bool())?;
    if !success {
        return None;
    }

    let inner = parsed.pointer("/data/proof/proof")?;
    let public_values_hex = inner
        .get("public_values")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;

    let field_commitments_meta = inner
        .get("field_commitments")
        .cloned()
        .or_else(|| {
            parsed
                .pointer("/data/proof/field_commitments_json")
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        });

    Some(ProofMaterial {
        public_values_hex,
        vk_hash: inner
            .get("vk_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        program_id: inner
            .get("program_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        field_commitments_meta,
    })
}

/// Fetch the full stored proof and extract its verification material.
///
/// Returns `Ok(None)` when the attester reports the proof does not exist (so a
/// caller can fall back), `Err` on transport / shape problems, and `Ok(Some)`
/// when the inner `data.proof.proof` blob was found.
async fn fetch_proof_material_by_id(
    client: &reqwest::Client,
    attester_url: &str,
    proof_id: &str,
) -> Result<Option<ProofMaterial>, String> {
    let url = format!(
        "{}/proofs/{}",
        attester_url.trim_end_matches('/'),
        proof_id
    );

    let resp = client.get(&url).send().await.map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] fetching full proof failed proof_id={} err={}",
            proof_id, e
        )
    })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!(
            "[FARM-NVM][ZPI] attester returned HTTP {} for full proof proof_id={}: {}",
            status, proof_id, redact_body_text(&body)
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] full proof response was not JSON proof_id={} err={} body={}",
            proof_id, e, redact_body_text(&body)
        )
    })?;

    let success = parsed
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !success {
        let err = parsed.get("error").and_then(|v| v.as_str()).unwrap_or("");
        if err.to_lowercase().contains("not found") {
            return Ok(None);
        }
        return Err(format!(
            "[FARM-NVM][ZPI] attester reported success=false for full proof proof_id={} error={}",
            proof_id, err
        ));
    }

    extract_proof_material_from_json(&parsed).ok_or_else(|| {
        format!(
            "[FARM-NVM][ZPI] full proof missing verification material proof_id={} body={}",
            proof_id, redact_body_text(&body)
        )
    }).map(Some)
}

/// Parse the SP1 `public_values` the zkVM guest committed.
///
/// Layout (must match `zero-proof-intent/src/zkp/commitment.rs`):
///   1. `[u8; 32]`                  — intent_commitment (raw, skipped here)
///   2. `u32`                       — verified_count (bincode default, fixint LE)
///   3. `String`                    — external_id
///   4. `Vec<(String, [u8; 32])>`   — field_name → commitment pairs
///
/// Returns `(committed_external_id, { field_name → commitment_hex })`.
fn parse_committed_public_values(
    public_values_hex: &str,
) -> Result<(String, std::collections::HashMap<String, String>), String> {
    use std::io::Read as _;

    let bytes = hex::decode(public_values_hex.trim().trim_start_matches("0x"))
        .map_err(|e| format!("[FARM-NVM][ZPI] public_values hex decode failed: {e}"))?;
    let mut cursor = std::io::Cursor::new(bytes);

    let mut _intent = [0u8; 32];
    cursor
        .read_exact(&mut _intent)
        .map_err(|e| format!("[FARM-NVM][ZPI] public_values: reading intent_commitment: {e}"))?;

    let _verified_count: u32 = bincode::deserialize_from(&mut cursor)
        .map_err(|e| format!("[FARM-NVM][ZPI] public_values: reading verified_count: {e}"))?;

    let committed_external_id: String = bincode::deserialize_from(&mut cursor)
        .map_err(|e| format!("[FARM-NVM][ZPI] public_values: reading external_id: {e}"))?;

    let pairs: Vec<(String, [u8; 32])> = bincode::deserialize_from(&mut cursor)
        .map_err(|e| format!("[FARM-NVM][ZPI] public_values: reading field_commitments: {e}"))?;

    let commitments = pairs
        .into_iter()
        .map(|(field, hash)| (field, hex::encode(hash)))
        .collect();

    Ok((committed_external_id, commitments))
}

/// SHA-256(field_name : canonical_value : external_id : secret_salt).
/// Mirrors `compute_field_commitment` in zero-proof-intent's commitment.rs.
fn compute_field_commitment(
    field_name: &str,
    canonical_value: &str,
    external_id: &str,
    secret_salt: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(field_name.as_bytes());
    hasher.update(b":");
    hasher.update(canonical_value.as_bytes());
    hasher.update(b":");
    hasher.update(external_id.as_bytes());
    hasher.update(b":");
    hasher.update(secret_salt.as_bytes());
    hex::encode(hasher.finalize())
}

/// Fetch the per-transaction salt from the attester so the merchant can
/// reproduce the same field commitments the prover used.
/// `GET /programs/{program_id}/derive-salt?external_id=` → `{ "derived_salt": "<hex>" }`.
async fn derive_salt_from_attester(
    client: &reqwest::Client,
    attester_url: &str,
    program_id: &str,
    external_id: &str,
) -> Result<String, String> {
    // program_id is content-addressed like `sha256:<hex>`; the ':' must be
    // percent-encoded so it isn't treated as a path component.
    let encoded_program = program_id.replace(':', "%3A");
    let url = format!(
        "{}/programs/{}/derive-salt?external_id={}",
        attester_url.trim_end_matches('/'),
        encoded_program,
        external_id
    );

    let resp = client.get(&url).send().await.map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] derive-salt request failed program_id={} external_id={} err={}",
            program_id, external_id, e
        )
    })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "[FARM-NVM][ZPI] derive-salt returned HTTP {} program_id={} external_id={}: {}",
            status, program_id, external_id, redact_body_text(&body)
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] derive-salt response not JSON program_id={} err={} body={}",
            program_id, e, redact_body_text(&body)
        )
    })?;

    parsed
        .get("derived_salt")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "[FARM-NVM][ZPI] derive-salt response missing `derived_salt` program_id={} body={}",
                program_id, body
            )
        })
}

/// Zero-trust checks against the zkVM-committed public values.
///
/// These do NOT cryptographically verify the Groth16 bytes (that would require
/// embedding the SP1 verifier + program ELF). They DO ensure the proof the
/// attester is vouching for actually binds to *this* transaction and *this*
/// amount, recomputed from the merchant's own record — closing the gap where
/// the merchant previously trusted the attester's opaque `verified` flag.
async fn run_zero_trust_proof_checks(
    client: &reqwest::Client,
    attester_url: &str,
    material: &ProofMaterial,
    external_id: &str,
    expected_amount_cents: u64,
    expected_currency: &str,
) -> Result<(), String> {
    let (committed_external_id, committed) =
        parse_committed_public_values(&material.public_values_hex)?;

    // (a) external_id binding — the proof MUST be for this exact transaction.
    if committed_external_id != external_id {
        return Err(format!(
            "[FARM-NVM][ZPI] external_id binding FAILED — proof commits external_id={} but we are settling external_id={}. Refusing (possible proof substitution / replay).",
            committed_external_id, external_id
        ));
    }
    tracing::info!(
        "[FARM-NVM][ZPI] external_id binding OK — proof commits external_id={}",
        committed_external_id
    );

    // (b) Optional VK / program pinning. If the operator pins an expected
    // verifying key or program id, enforce it; otherwise just surface what we
    // observed so a mismatch is visible in the logs.
    if let Some(observed) = material.vk_hash.as_deref() {
        match std::env::var("ZPI_EXPECTED_VK_HASH")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            Some(expected) if !expected.eq_ignore_ascii_case(observed) => {
                return Err(format!(
                    "[FARM-NVM][ZPI] vk_hash mismatch — expected {} but proof carries {}. Refusing (wrong verifier circuit).",
                    expected, observed
                ));
            }
            Some(_) => {
                tracing::info!("[FARM-NVM][ZPI] vk_hash matches pinned ZPI_EXPECTED_VK_HASH")
            }
            None => tracing::info!(
                "[FARM-NVM][ZPI] proof vk_hash={} (set ZPI_EXPECTED_VK_HASH to pin)",
                observed
            ),
        }
    }
    if let (Some(observed), Some(expected)) = (
        material.program_id.as_deref(),
        std::env::var("ZPI_EXPECTED_PROGRAM_ID")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    ) {
        if expected != observed {
            return Err(format!(
                "[FARM-NVM][ZPI] program_id mismatch — expected {} but proof carries {}. Refusing.",
                expected, observed
            ));
        }
        tracing::info!("[FARM-NVM][ZPI] program_id matches pinned ZPI_EXPECTED_PROGRAM_ID");
    }

    // Defence in depth: the attester also stores a flat JSON copy of the field
    // commitments. It should agree with the committed bytes; warn (don't fail)
    // if it drifts, since the committed public_values are authoritative.
    if let Some(meta) = material.field_commitments_meta.as_ref().and_then(|v| v.as_array()) {
        for entry in meta {
            if let (Some(field), Some(commitment)) = (
                entry.get("field").and_then(|v| v.as_str()),
                entry.get("commitment").and_then(|v| v.as_str()),
            ) {
                if let Some(committed_hex) = committed.get(field) {
                    if !committed_hex.eq_ignore_ascii_case(commitment) {
                        tracing::warn!(
                            "[FARM-NVM][ZPI] attester metadata field_commitment for `{}` disagrees with committed public_values (meta={} committed={}) — trusting committed bytes",
                            field, commitment, committed_hex
                        );
                    }
                }
            }
        }
    }

    // (c) Amount + currency binding — recompute merchant-owned commitments from
    // checkout state and require them to match the proof.
    // `amount_cents` is already integer cents, which is exactly the canonical
    // form `canonicalize_value("total_amount", ..)` produces.
    let canonical_amount = expected_amount_cents.to_string();
    let canonical_currency = expected_currency.trim().to_ascii_uppercase();
    let committed_amount_hash = match committed.get("total_amount") {
        Some(hash) => hash,
        None => {
            let committed_fields: Vec<&String> = committed.keys().collect();
            if zpi_require_amount_commitment() {
                return Err(format!(
                    "[FARM-NVM][ZPI] proof does not commit a `total_amount` field (committed fields: {:?}) — refusing (amount ZK binding required by default; set ZPI_REQUIRE_AMOUNT_COMMITMENT=false to opt out).",
                    committed_fields
                ));
            }
            tracing::warn!(
                "[FARM-NVM][ZPI] proof commits no `total_amount` field (committed: {:?}) — amount not ZK-verified, relying on external_id binding only (ZPI_REQUIRE_AMOUNT_COMMITMENT=false).",
                committed_fields
            );
            ""
        }
    };
    let committed_currency_hash = committed.get("currency").ok_or_else(|| {
        let committed_fields: Vec<&String> = committed.keys().collect();
        format!(
            "[FARM-NVM][ZPI] proof does not commit a `currency` field (committed fields: {:?}) — refusing (currency ZK binding required).",
            committed_fields
        )
    })?;
    let program_id = material.program_id.as_deref().ok_or_else(|| {
        format!(
            "[FARM-NVM][ZPI] cannot verify amount/currency: proof record has no program_id to derive the salt (external_id={})",
            external_id
        )
    })?;
    let salt = derive_salt_from_attester(client, attester_url, program_id, external_id).await?;

    if !committed_amount_hash.is_empty() {
        let recomputed =
            compute_field_commitment("total_amount", &canonical_amount, external_id, &salt);

        if !recomputed.eq_ignore_ascii_case(committed_amount_hash) {
            return Err(format!(
                "[FARM-NVM][ZPI] amount binding FAILED — proof's total_amount commitment {} does not match the {}-cent charge re-hashed from our record (got {}). Refusing (amount tampering).",
                committed_amount_hash, expected_amount_cents, recomputed
            ));
        }
        tracing::info!(
            "[FARM-NVM][ZPI] amount binding OK — proof commits total_amount == {} cents",
            expected_amount_cents
        );
    }

    let recomputed = compute_field_commitment("currency", &canonical_currency, external_id, &salt);
    if !recomputed.eq_ignore_ascii_case(committed_currency_hash) {
        return Err(proof_binding_error(
            "currency",
            &canonical_currency,
            "Re-prove with spend witness currency matching merchant canonical form (trim + uppercase ISO 4217, e.g. USD). Proofs that committed lowercase or mixed-case currency (e.g. usd) will not bind against merchant USD.",
            &format!(
                "proof currency commitment {} does not match merchant re-hash for canonical '{}' (recomputed {}).",
                committed_currency_hash, canonical_currency, recomputed
            ),
        ));
    }
    tracing::info!(
        "[FARM-NVM][ZPI] currency binding OK — proof commits currency == {}",
        canonical_currency
    );

    Ok(())
}

async fn verify_sp1_proof_with_attester(
    client: &reqwest::Client,
    attester_url: &str,
    proof_id: &str,
    external_id: &str,
    expected_amount_cents: u64,
    expected_currency: &str,
    expected_program_id: Option<&str>,
    expected_vk_hash: Option<&str>,
) -> Result<(), String> {
    let program_id = expected_program_id.ok_or_else(|| {
        format!(
            "[FARM-NVM][ZPI] cannot request attester crypto verification: proof_id={} has no program_id",
            proof_id
        )
    })?;
    let salt = derive_salt_from_attester(client, attester_url, program_id, external_id).await?;
    let mut expected_field_commitments = BTreeMap::new();
    expected_field_commitments.insert(
        "total_amount".to_string(),
        compute_field_commitment(
            "total_amount",
            &expected_amount_cents.to_string(),
            external_id,
            &salt,
        ),
    );
    expected_field_commitments.insert(
        "currency".to_string(),
        compute_field_commitment(
            "currency",
            &expected_currency.trim().to_ascii_uppercase(),
            external_id,
            &salt,
        ),
    );

    let url = format!(
        "{}/proofs/attester-verify",
        attester_url.trim_end_matches('/')
    );
    let mut payload = json!({
        "proof_id": proof_id,
        "expected_external_id": external_id,
        "expected_field_commitments": expected_field_commitments,
        "expected_program_id": program_id,
    });
    if let Some(vk_hash) = expected_vk_hash {
        payload["expected_vk_hash"] = json!(vk_hash);
    }

    let resp = client.post(&url).json(&payload).send().await.map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] attester crypto verification request failed proof_id={} err={}",
            proof_id, e
        )
    })?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "[FARM-NVM][ZPI] attester crypto verification HTTP {} proof_id={}: {}",
            status,
            proof_id,
            redact_body_text(&body)
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] attester crypto verification response was not JSON proof_id={} err={} body={}",
            proof_id,
            e,
            redact_body_text(&body)
        )
    })?;

    let success = parsed
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let crypto_verified = parsed
        .get("crypto_verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let bindings_verified = parsed
        .get("bindings_verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !(success && crypto_verified && bindings_verified) {
        return Err(format!(
            "[FARM-NVM][ZPI] attester crypto verification failed proof_id={} response={}",
            proof_id,
            redact_json_value(&parsed)
        ));
    }

    tracing::info!(
        "[FARM-NVM][ZPI] attester SP1 verification OK proof_id={} external_id={} fields=[total_amount,currency]",
        proof_id,
        external_id
    );
    Ok(())
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
            status, proof_id, redact_body_text(&body)
        ));
    }

    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return AttesterVerifyOutcome::Failed(format!(
                "[FARM-NVM][ZPI] attester response was not JSON proof_id={} err={} body={}",
                proof_id, e, redact_body_text(&body)
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
            status, external_id, redact_body_text(&body)
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "[FARM-NVM][ZPI] attester session response was not JSON external_id={} err={} body={}",
            external_id, e, redact_body_text(&body)
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

/// Delta #5 internal endpoint — ZPI-ZKPay calls this over localhost to confirm
/// the merchant verified a ZPI proof for `external_id` before it mints an x402
/// token. Localhost-only trust, mirroring the existing `token_ref` channel
/// (agent-b → ZPI-ZKPay `GET /tokens/:ref`); no shared secret.
///
/// `GET /internal/intent-verified?external_id=...` →
/// `{ verified, amount_cents?, merchant_url?, verified_at_secs? }`.
pub async fn handle_intent_verified(
    State(state): State<SharedFarmState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let external_id = params
        .get("external_id")
        .map(|s| s.trim())
        .unwrap_or_default();

    if external_id.is_empty() {
        return Json(json!({ "verified": false, "reason": "external_id query param required" }));
    }

    let mut state = state.write().await;
    state.prune_verified_intents();

    match state.verified_intents.get(external_id) {
        Some(v) => {
            let verified_at_secs = v
                .verified_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Json(json!({
                "verified": true,
                "external_id": external_id,
                "amount_cents": v.amount_cents,
                "currency": v.currency,
                "merchant_url": v.merchant_url,
                "proof_id": v.proof_id,
                "verified_at_secs": verified_at_secs,
            }))
        }
        None => Json(json!({
            "verified": false,
            "external_id": external_id,
            "reason": "no merchant verification on record (call pay-with-nevermined with verify_only=true first, or it expired)",
        })),
    }
}

pub async fn handle_pay_with_nevermined(
    State(state): State<SharedFarmState>,
    Json(req): Json<PayWithNeverminedRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    allowed_merchant_host(&req.merchant_url)
        .map_err(|e| (StatusCode::FORBIDDEN, Json(FarmToolResponse::err(403, e))))?;

    let amount_cents = amount_to_cents(req.amount)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(FarmToolResponse::err(400, e))))?;

    let canonical_currency = canonicalize_merchant_currency(req.currency.as_deref());

    let requested_external_id = req.external_id.clone();
    let zpi_proof = req.zpi_proof.clone().unwrap_or_default();

    // ── Delta #5 — verify-only mode ───────────────────────────────────────
    // The merchant verifies the ZPI proof against the attester and records the
    // outcome, WITHOUT minting or settling. ZPI-ZKPay confirms this record
    // over localhost before it mints an x402 token, so a credential is never
    // created for an intent the merchant has not validated.
    if req.verify_only.unwrap_or(false) {
        let external_id = requested_external_id.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    "external_id is required for verify_only".to_string(),
                )),
            )
        })?;
        let supplied_proof_id = proof_id_from_request(&req).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    "proof_id is required for verify_only so the merchant can verify the proof against the attester".to_string(),
                )),
            )
        })?;
        if supplied_proof_id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    "proof_id is required for verify_only so the merchant can verify the proof against the attester".to_string(),
                )),
            ));
        }

        let canonical_proof_id = verify_zpi_proof_against_attester(
            Some(supplied_proof_id),
            &external_id,
            amount_cents,
            &canonical_currency,
            &req.merchant_url,
        )
        .await
        .map_err(|e| {
            tracing::error!("{}", e);
            (
                StatusCode::FORBIDDEN,
                Json(proof_verification_error_response(403, e)),
            )
        })?;

        {
            let mut state = state.write().await;
            state.prune_verified_intents();
            state.verified_intents.insert(
                external_id.clone(),
                super::state::VerifiedIntent {
                    amount_cents,
                    currency: canonical_currency.clone(),
                    merchant_url: req.merchant_url.clone(),
                    proof_id: canonical_proof_id.clone(),
                    verified_at: std::time::SystemTime::now(),
                },
            );
        }

        tracing::info!(
            "[FARM-NVM][ZPI] verify_only OK — recorded merchant verification external_id={} amount_cents={} currency={}",
            external_id,
            amount_cents,
            canonical_currency
        );

        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "INTENT_VERIFIED",
            "external_id": external_id,
            "proof_id": canonical_proof_id,
            "amount": format_dollars(amount_cents),
            "amount_cents": amount_cents,
            "currency": canonical_currency,
            "merchant_url": req.merchant_url,
            "instructions": "Merchant verified the ZPI proof. Now call zpi-zkpay pay-with-nevermined-merchant-settles Phase 2 (same external_id + proof_id) to mint — only after this INTENT_VERIFIED. Then call this tool again with token_ref + plan_id + external_id + proof_id to settle."
        }))));
    }

    // The preferred flow has Claude forward only token_ref; agent-b fetches the
    // minted token from ZPI-ZKPay over localhost. In that case the raw zpi_proof
    // bytes are not required — the proof's validity is established by proof_id
    // against the attester. The legacy flow still requires the bytes so this
    // handler can stash pending state and mint locally using NVM_API_KEY.
    let has_supplied_token = req
        .token_ref
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || req
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
                order_id: String::new(),
            },
        );

        return Ok(Json(FarmToolResponse::ok(json!({
            "status": "NEEDS_INTENT_PROOF",
            "external_id": external_id,
            "intent_type": "spend",
            "payment_details": {
                "amount": format_dollars(amount_cents),
                "amount_cents": amount_cents,
                "currency": canonical_currency,
                "merchant_url": req.merchant_url,
                "description": req.description,
                "payment_processor": "nevermined"
            },
            "instructions": "LEGACY ENTRYPOINT ONLY — after merchant checkout, follow the merchant-settles flow instead. Preferred flow: (1) zpi-zkpay pay-with-nevermined-merchant-settles Phase 1 with this external_id (no proof_id) to seed zkpay; (2) chp_save + prove_intent(intent_type='spend') + poll for proof_id; (3) call this merchant tool with verify_only=true + external_id + proof_id so the merchant returns INTENT_VERIFIED; (4) zpi-zkpay Phase 2 mint; (5) call this merchant tool with token_ref + plan_id + external_id + proof_id to settle. LEGACY fallback: call pay-with-nevermined again with zpi_proof + external_id (merchant mints using its own NVM_API_KEY)."
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
                    order_id: pending.order_id.clone(),
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
    // In the preferred flow proof_id is mandatory — the whole point is
    // that the merchant verifies the proof itself rather than trusting Claude
    // or ZPI-ZKPay blindly. In the legacy flow we soft-skip if it's missing
    // so older demos still work (with a visible warning emitted downstream).
    let supplied_proof_id = proof_id_from_request(&req);

    if has_supplied_token && supplied_proof_id.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                "proof_id is required when token_ref or legacy x402 token fields are supplied so the merchant can verify the ZPI intent proof against the attester".to_string(),
            )),
        ));
    }

    let canonical_proof_id = verify_zpi_proof_against_attester(
        supplied_proof_id,
        &external_id,
        amount_cents,
        &canonical_currency,
        &req.merchant_url,
    )
    .await
    .map_err(|e| {
        tracing::error!("{}", e);
        (
            StatusCode::FORBIDDEN,
            Json(proof_verification_error_response(403, e)),
        )
    })?;

    // ── Resolve the payment credential ───────────────────────────────────
    // ZPI-ZKPay owns /x402/permissions (it holds the user's NVM API key).
    // When `token_ref` is present, fetch the full token directly from
    // ZPI-ZKPay over localhost — this eliminates the ~20% corruption rate
    // when Claude relays the ~1.1 KB base64 token through tool-call args.
    // Fall back to `x402_access_token` / `payload_encoded` / legacy mint.
    let resolved_from_ref = if let Some(ref tref) = req.token_ref {
        let tref = tref.trim();
        if !tref.is_empty() {
            let zkpay_port = std::env::var("ZKPAY_PORT").unwrap_or_else(|_| "3002".to_string());
            let url = format!("http://localhost:{}/tokens/{}", zkpay_port, tref);
            tracing::info!(
                "[FARM-NVM] Resolving token_ref={} from ZPI-ZKPay at {} external_id={}",
                tref, url, external_id
            );
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            let token = body.get("x402_access_token")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            if token.is_some() {
                                tracing::info!(
                                    "[FARM-NVM] token_ref={} resolved successfully — using ZPI-ZKPay-fetched token (corruption-proof) external_id={}",
                                    tref, external_id
                                );
                            }
                            token
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[FARM-NVM] token_ref={} response parse failed: {} — falling back to legacy relay token fields external_id={}",
                                tref, e, external_id
                            );
                            None
                        }
                    }
                }
                Ok(resp) => {
                    tracing::warn!(
                        "[FARM-NVM] token_ref={} returned {} — falling back to legacy relay token fields external_id={}",
                        tref, resp.status(), external_id
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        "[FARM-NVM] token_ref={} fetch failed: {} — falling back to legacy relay token fields external_id={}",
                        tref, e, external_id
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let supplied_token = resolved_from_ref
        .or_else(|| {
            req.x402_access_token
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            req.payload_encoded
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    let (payment_credential, x_nevermined_api_key, token_supplied_by_zkpay) = match supplied_token {
        Some(token) => {
            tracing::info!(
                "[FARM-NVM] Using x402 token resolved from token_ref or legacy fallback (mint skipped) external_id={} token={}",
                external_id,
                redact_secret(&token)
            );
            (token, String::new(), true)
        }
        None => {
            tracing::warn!(
                "[FARM-NVM] No token_ref or raw x402 token supplied — falling back to legacy mint via NVM_API_KEY. \
                 Update Claude's flow to call zpi-zkpay's pay-with-nevermined-merchant-settles first \
                 and forward token_ref + plan_id here. external_id={}",
                external_id
            );
            let nvm_api_key = std::env::var("NVM_API_KEY").map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(FarmToolResponse::err(
                        500,
                        "token_ref/x402 token is missing and NVM_API_KEY is not set — cannot proceed".to_string(),
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
                    .map_err(|e| (StatusCode::BAD_GATEWAY, Json(FarmToolResponse::err(502, e))))?
            } else {
                nvm_api_key.clone()
            };
            (cred, nvm_api_key, false)
        }
    };

    // The minted x402 token carries a one-shot redeem authorization: Nevermined
    // /verify consumes it, so a second /verify on the same token fails with
    // BCK.X402.0005 "invalid signature" (confirmed: byte-identical token,
    // verified once by ZPI-ZKPay, rejected on the next /verify). When ZPI-ZKPay
    // supplied the token we therefore must NOT preflight-verify here — the single
    // authoritative /verify + /settle happens once in handle_checkout_nevermined.
    if token_supplied_by_zkpay {
        tracing::info!(
            "[FARM-NVM] Skipping pay-with-nevermined preflight /verify (token from ZPI-ZKPay; checkout-nevermined will verify + settle once) external_id={}",
            external_id
        );
    } else {
        let plan_id_for_verify = req
            .plan_id
            .as_deref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| extract_plan_id_from_x402_token(&payment_credential));

        verify_nevermined_token_if_configured(
            &payment_credential,
            amount_cents,
            &req.merchant_url,
            plan_id_for_verify.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(FarmToolResponse::err(502, e))))?;
    }

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
        .header("X-ZPI-External-Id", &external_id)
        .header("X-ZPI-Currency", &canonical_currency);
    if !canonical_proof_id.is_empty() {
        merchant_req = merchant_req.header("X-ZPI-Proof-Id", &canonical_proof_id);
    }
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
        "proof_id": canonical_proof_id,
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
        "instructions": "Now call zpi-zkpay MCP tool pay-with-credit-card with zpi_arguments exactly. If that succeeds, call confirm-payment with order_id, payment_confirmed=true, and zpi_response from zpi-zkpay.",
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

    // Require a zpi_response containing a charge_bundle — this is set by
    // zpi-zkpay checkout-with-credit-card and proves an actual charge was made.
    // Without this check an LLM can mark orders as PAID without charging the card.
    let has_charge_evidence = req.zpi_response.as_ref().and_then(|v| v.get("charge_bundle")).is_some()
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
                "zpi_response from checkout-with-credit-card is required (must contain charge_bundle or external_id)".to_string(),
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
                .zpi_response
                .as_ref()
                .and_then(|v| v.get("charge_bundle"))
                .and_then(|v| v.as_str())
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

                        let dpan = payload.get("dpan").and_then(|v| v.as_str()).unwrap_or("");
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
                            let cavv = payload.get("cavv").and_then(|v| v.as_str());
                            let eci = payload.get("eci").and_then(|v| v.as_str());
                            let ds_trans_id =
                                payload.get("dsTransId").and_then(|v| v.as_str());
                            let bundle_merchant_id = payload
                                .get("merchant_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let bundle_ext_id = payload
                                .get("external_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or(req.external_id.as_deref().unwrap_or(""));

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
                                        req.order_id,
                                        pi_id
                                    );
                                    Some(pi_id)
                                }
                                Err(e) => {
                                    tracing::error!("[FARM-VGS] Stripe charge failed: {}", e);
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
                        tracing::error!("[FARM-VGS] JWE decrypt failed — refusing payment: {}", e);
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

// ── settle-via-nevermined: JWE decryption + NVM settle / PSP forwarding ─────

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
    State(state): State<SharedFarmState>,
    State(db): State<SharedMerchantDb>,
    Json(req): Json<FarmConfirmPaymentRequest>,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    tracing::info!(
        "[FARM-CONFIRM] settle-via-nevermined called: merchant_id={}, external_id={}",
        req.merchant_id,
        req.external_id,
    );

    // 1. Decode JWE protected header and check kid
    let mut req = req;
    let header = decode_jwe_header(&req.charge_bundle).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(400, format!("Invalid charge_bundle: {e}"))),
        )
    })?;
    // Lean Nevermined flow calls this with only { charge_bundle, external_id }.
    // Resolve merchant_id from the bundle's kid (authoritative) when omitted,
    // falling back to the merchant's registered id.
    if req.merchant_id.trim().is_empty() {
        req.merchant_id = header
            .get("kid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                std::env::var("ZPI_VGS_MERCHANT_ID")
                    .unwrap_or_else(|_| "test-merchant-p2".to_string())
            });
    }
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
    .map_err(|e| (StatusCode::BAD_REQUEST, Json(FarmToolResponse::err(400, e))))?;

    // 4. Parse decrypted payload
    let decrypted: serde_json::Value = serde_json::from_slice(&plaintext_bytes).map_err(|e| {
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

    // 5b. Reject stale bundles (> 5 minutes old) to prevent replay attacks —
    // mirrors the VGS charge_bundle freshness check.
    if let Some(ts) = decrypted.get("issuedAt").and_then(|v| v.as_str()) {
        if let Ok(issued) = ts.parse::<chrono::DateTime<chrono::Utc>>() {
            let age_secs = (chrono::Utc::now() - issued).num_seconds();
            if age_secs > 300 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(FarmToolResponse::err(
                        400,
                        format!("charge_bundle is stale ({age_secs}s old, max 300s)"),
                    )),
                ));
            }
        }
    }

    // 5c. Branch on the credential mode embedded in the decrypted bundle.
    // "nevermined_x402" → the merchant re-checks the ZPI proof against the
    // attester, then verifies + settles the x402 token directly with Nevermined
    // (mirrors the VGS path which charges the PSP directly). Any other mode
    // (default / "cmp_dpan") keeps the existing PSP /charge forwarding.
    let payment_credential_mode = decrypted
        .get("paymentCredentialMode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if payment_credential_mode == "nevermined_x402" {
        return handle_confirm_nevermined_x402(state, db, &req, &decrypted).await;
    }

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

/// Settle a decrypted `nevermined_x402` charge_bundle.
///
/// The producer (ZPI-ZKPay) has already minted the user's x402 access token and
/// sealed it (plus the ZPI proof metadata) into the JWE we just decrypted. The
/// merchant now, with no further Claude round-trips:
///   1. re-checks the ZPI intent proof against the attester (zero-trust), then
///   2. verifies + settles the x402 token directly with Nevermined, and
///   3. marks the order PAID.
/// This mirrors the VGS path, which charges the PSP directly from the bundle.
async fn handle_confirm_nevermined_x402(
    state: SharedFarmState,
    db: SharedMerchantDb,
    req: &FarmConfirmPaymentRequest,
    decrypted: &serde_json::Value,
) -> Result<Json<FarmToolResponse>, (StatusCode, Json<FarmToolResponse>)> {
    let external_id = req.external_id.clone();

    // (a) Extract the credential + binding fields from the decrypted bundle.
    let token = decrypted
        .get("x402AccessToken")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    "charge_bundle is missing x402AccessToken".to_string(),
                )),
            )
        })?
        .to_string();
    let proof_id = decrypted
        .get("proof_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let plan_id = decrypted
        .get("planId")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let amount_cents = decrypted
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(FarmToolResponse::err(
                    400,
                    "charge_bundle is missing a numeric amount (cents)".to_string(),
                )),
            )
        })?;
    let currency = canonicalize_merchant_currency(
        decrypted.get("currency").and_then(|v| v.as_str()),
    );

    // (b) external_id was already validated against the bundle in step 5.

    // Resolve the order this intent belongs to. The bundle does not carry an
    // order_id, so we look it up via the pending-intent record stashed at
    // checkout (keyed by external_id), which also gives us the canonical
    // merchant resource URL for the Nevermined verify/settle calls.
    let pending = {
        let st = state.read().await;
        st.pending_nevermined.get(&external_id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                format!("No pending Nevermined payment for external_id={external_id}"),
            )),
        )
    })?;
    let order_id = pending.order_id.clone();
    let resource_url = pending.merchant_url.clone();

    let order = {
        let st = state.read().await;
        st.orders.get(&order_id).cloned()
    }
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(FarmToolResponse::err(
                404,
                format!("Order '{order_id}' not found for external_id={external_id}"),
            )),
        )
    })?;

    if order.status == OrderStatus::Paid {
        return Err((
            StatusCode::CONFLICT,
            Json(FarmToolResponse::err(409, "Order already paid".into())),
        ));
    }

    if amount_cents != order.total_cents {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                format!(
                    "charge_bundle amount mismatch: bundle={amount_cents}, order={}",
                    order.total_cents
                ),
            )),
        ));
    }

    // (c) Optional zero-trust ZPI proof re-check against the attester. No Claude
    // step: the merchant pulls the proof itself and asserts it binds to this
    // external_id + amount + currency before touching the payment rail. This is
    // optional per the Nevermined design (toggle with ZPI_MERCHANT_VERIFY); when
    // disabled the merchant trusts ZPI-ZKPay's pre-mint proof gate + the JWE
    // trust boundary. Applies in both intent and proof verification modes.
    if zpi_merchant_verify_enabled() {
        verify_zpi_proof_against_attester(
            proof_id.as_deref(),
            &external_id,
            amount_cents,
            &currency,
            &resource_url,
        )
        .await
        .map_err(|e| {
            tracing::error!("[FARM-CONFIRM][NVM] ZPI proof verification failed: {}", e);
            (
                StatusCode::FORBIDDEN,
                Json(FarmToolResponse::err(403, e)),
            )
        })?;
    } else {
        tracing::warn!(
            "[FARM-CONFIRM][NVM] merchant ZPI intent re-verification DISABLED (ZPI_MERCHANT_VERIFY=off) for external_id={} — trusting the zkpay proof gate + JWE boundary",
            external_id
        );
    }

    // (d) Verify then settle the x402 token directly with Nevermined.
    verify_nevermined_token_if_configured(
        &token,
        amount_cents,
        &resource_url,
        plan_id.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!("[FARM-CONFIRM][NVM] Nevermined verify failed: {}", e);
        (
            StatusCode::BAD_GATEWAY,
            Json(FarmToolResponse::err(502, e)),
        )
    })?;

    let tx_hash = settle_nevermined_token(
        &token,
        amount_cents,
        &resource_url,
        plan_id.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::error!("[FARM-CONFIRM][NVM] Settle failed — order NOT marked paid: {}", e);
        (
            StatusCode::BAD_GATEWAY,
            Json(FarmToolResponse::err(502, format!("Payment settlement failed: {e}"))),
        )
    })?
    .unwrap_or_else(|| {
        format!(
            "nvm-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("tx")
        )
    });

    // (e) Mark the order paid — same shape as the VGS / x402 confirm paths.
    {
        let mut st = state.write().await;
        if let Some(existing) = st.orders.get_mut(&order_id) {
            existing.status = OrderStatus::Paid;
        }
        st.carts.remove(&order.session_id);
        st.pending_nevermined.remove(&external_id);
    }

    if let Err(e) =
        db.update_order_status(&order_id, &OrderStatus::Paid, Some(&tx_hash), Some("nevermined-card"))
    {
        tracing::error!("[FARM-CONFIRM][NVM] Failed to persist paid status for {}: {}", order_id, e);
    }
    if let Err(e) =
        db.update_order_zpi_audit(&order_id, Some(&external_id), None, proof_id.as_deref())
    {
        tracing::error!("[FARM-CONFIRM][NVM] Failed to persist ZPI audit IDs for {}: {}", order_id, e);
    }

    tracing::info!(
        "[FARM-CONFIRM][NVM] order PAID order_id={} external_id={} tx_hash={} amount_cents={} currency={}",
        order_id,
        external_id,
        tx_hash,
        amount_cents,
        currency,
    );

    // (f) Success response — order confirmed, tx id, credits.
    let credits_used: i64 = std::env::var("NEVERMINED_CREDITS_PER_REQUEST")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(1);

    Ok(Json(FarmToolResponse::ok(json!({
        "status": "PAID",
        "order_confirmed": true,
        "order_id": order_id,
        "external_id": external_id,
        "merchant_id": req.merchant_id.clone(),
        "payment_processor": "nevermined",
        "payment_credential_mode": "nevermined_x402",
        "transaction_ref": tx_hash.clone(),
        "tx_hash": tx_hash,
        "proof_id": proof_id,
        "credits_used": credits_used,
        "amount": format_dollars(amount_cents),
        "amount_cents": amount_cents,
        "currency": currency,
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

    add_to_cart(cart, &req.product_id, req.quantity)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(FarmToolResponse::err(400, e))))?;

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
        let amount_cents = order.total_cents;
        // Merchant identifier the merchant registers its public key under — same
        // id used by the VGS flow so ZPI-ZKPay can look up the merchant's JWK.
        let merchant_id = std::env::var("VGS_MERCHANT_ID")
            .unwrap_or_else(|_| "test-merchant-p2".to_string());

        state.pending_nevermined.insert(
            external_id.clone(),
            super::state::PendingNeverminedPayment {
                merchant_url: merchant_url.clone(),
                amount_cents,
                description: format!("Farm order {}", order.order_id),
                order_id: order_id.clone(),
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
                "external_id": external_id.clone(),
                "intent_type": "spend",
                "merchant_id": merchant_id.clone(),
                "merchant_url": merchant_url.clone(),
                "payment_details": {
                    "amount": format_dollars(amount_cents),
                    "amount_cents": amount_cents,
                    "currency": "USD",
                    "description": format!("Farm order {}", order.order_id)
                },
                "next_action": {
                    "tool_server": "zpi-zkpay",
                    "tool": "pay-with-nevermined",
                    "arguments": {
                        "amount": amount_cents,
                        "currency": "USD",
                        "merchant_id": merchant_id.clone(),
                        "external_id": external_id.clone(),
                        "merchant_url": merchant_url.clone()
                    }
                },
                "instructions": "Canonical external_id for this payment - reuse it everywhere. (1) chp_save. (2) prove_intent(this external_id, intent_type=spend). (3) zpi-zkpay pay-with-nevermined { amount: amount_cents, currency, merchant_id, external_id, merchant_url } -> returns charge_bundle. If pay-with-nevermined returns proof_not_ready/PENDING_PROOF, poll zpi_get_zkp_status until READY then retry pay-with-nevermined; otherwise proceed. (4) settle-via-nevermined { charge_bundle, external_id } to settle."
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
            "description": "Checkout the cart. Supports x402 crypto, Nevermined card demo flow, or VGS card flow. x402 returns HTTP 402 payment challenge. nevermined_card returns NEEDS_INTENT_PROOF with a canonical UUIDv4 external_id plus merchant_id; follow FARM_INSTRUCTIONS step 8 (chp_save → prove_intent → zpi-zkpay pay-with-nevermined returns a charge_bundle → settle-via-nevermined to settle). vgs_card returns a proof step and follow-up tool call.",
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
            "description": "Prepare VGS checkout after proof validation. This tool does NOT charge directly. It returns zpi-zkpay MCP arguments for checkout-with-credit-card. After zpi-zkpay payment succeeds, call confirm-payment to finalize order status.",
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
            "description": "Finalize farm order after zpi-zkpay checkout-with-credit-card succeeds. Marks the order as PAID and stores transaction reference.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "order_id": {
                        "type": "string",
                        "description": "Order ID returned from farm-checkout with payment_method='vgs_card'."
                    },
                    "payment_confirmed": {
                        "type": "boolean",
                        "description": "Must be true only after zpi-zkpay payment succeeded."
                    },
                    "transaction_ref": {
                        "type": "string",
                        "description": "Optional transaction reference to persist with order status."
                    },
                    "external_id": {
                        "type": "string",
                        "description": "Optional external ID used for proof/idempotency."
                    },
                    "zpi_response": {
                        "type": "object",
                        "description": "Optional zpi-zkpay checkout-with-credit-card response payload."
                    }
                },
                "required": ["order_id", "payment_confirmed"]
            }
        }),
        json!({
            "name": "pay-with-nevermined",
            "description": "Complete a Nevermined merchant-settles card payment. Preferred flow: merchant checkout external_id -> zpi-zkpay merchant-settles Phase 1 seed -> ZPI proof -> this tool with verify_only=true -> zpi-zkpay Phase 2 mint -> this tool settle. This tool is used twice in the preferred flow: first with verify_only=true + external_id + proof_id to record INTENT_VERIFIED before mint, then with token_ref + plan_id + external_id + proof_id to settle. LEGACY FLOW: calling without token/proof returns NEEDS_INTENT_PROOF; calling with external_id + zpi_proof mints internally using the merchant's NVM_API_KEY env var.",
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
                        "description": "UUIDv4 external_id from farm-checkout NEEDS_INTENT_PROOF. Required for verify_only and settle calls."
                    },
                    "zpi_proof": {
                        "type": "string",
                        "description": "ZPI proof blob/string from prove_intent. Required for legacy mint flow only."
                    },
                    "proof_id": {
                        "type": "string",
                        "description": "Canonical proof_id from prove_intent. When supplied, the merchant pulls the proof from the attester (GET /proofs/{id}/verify) and rejects the payment if invalid."
                    },
                    "token_ref": {
                        "type": "string",
                        "description": "Short opaque ref (e.g. tref-…) from zpi-zkpay's pay-with-nevermined-merchant-settles response. Preferred settlement input — the merchant fetches the full token from ZPI-ZKPay over localhost, eliminating relay corruption."
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Nevermined planId from zpi-zkpay's payment_required.accepts[0].planId. Required for preferred token_ref settlement because the merchant's own NVM key (if any) charges against a different plan."
                    },
                    "x402_access_token": {
                        "type": "string",
                        "description": "Legacy/manual fallback raw x402 token. Preferred flow uses token_ref so Claude does not relay raw token strings."
                    },
                    "payload_encoded": {
                        "type": "string",
                        "description": "Legacy/manual fallback x402 payload_encoded. Preferred flow uses token_ref."
                    },
                    "verify_only": {
                        "type": "boolean",
                        "description": "When true, the merchant ONLY verifies the ZPI proof against the attester and records the result (no mint, no settle). Requires external_id + proof_id. Returns proof_id. Call this BEFORE asking zpi-zkpay to mint — zpi-zkpay confirms this verification before minting."
                    },
                    "currency": {
                        "type": "string",
                        "description": "ISO 4217 currency for ZPI amount/currency binding (e.g. USD). Trimmed and uppercased; defaults to USD when omitted. Must match checkout payment_details.currency and the spend proof witness."
                    }
                },
                "required": ["merchant_url", "amount", "description"]
            }
        }),
        json!({
            "name": "settle-via-nevermined",
            "description": "Merchant-side Nevermined settlement. Decrypts a JWE charge_bundle with the merchant's EC private key (ECDH-ES+A256KW / A256GCM) and validates the external_id. If the bundle's paymentCredentialMode is 'nevermined_x402', the merchant optionally re-checks the ZPI intent proof against the attester (toggle with ZPI_MERCHANT_VERIFY; default on), then verifies + settles the x402 token directly with Nevermined and marks the order PAID. Otherwise it forwards the plaintext payment bundle to the configured PSP endpoint. Returns PAID status with a summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "charge_bundle": {
                        "type": "string",
                        "description": "JWE compact serialization returned by zpi-zkpay (pay-with-nevermined or pay-with-credit-card)."
                    },
                    "external_id": {
                        "type": "string",
                        "description": "External payment ID to validate against the decrypted bundle."
                    },
                    "merchant_id": {
                        "type": "string",
                        "description": "Optional merchant identifier; must match the JWE kid header. When omitted, it is resolved from the bundle's kid."
                    },
                    "psp_provider": {
                        "type": "string",
                        "description": "Optional PSP provider name label (default: zpi-zkpay). PSP path only."
                    },
                    "psp_endpoint": {
                        "type": "string",
                        "description": "Optional override for the PSP charge endpoint URL. PSP path only."
                    }
                },
                "required": ["charge_bundle", "external_id"]
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
8. For the Nevermined flow (user's NVM API key stays inside ZPI-ZKPay):
    a. Merchant checkout returns NEEDS_INTENT_PROOF with a canonical UUIDv4 external_id,
       merchant_id, amount_cents and currency. This external_id is canonical — reuse it in
       every later step; do not create another.
    b. Call chp_save, then prove_intent with the same external_id and intent_type='spend'.
    c. Call zpi-zkpay pay-with-nevermined with { amount: amount_cents, currency, merchant_id,
       external_id } (exactly the checkout next_action.arguments). It mints the x402 token and
       returns an encrypted charge_bundle JWE. If pay-with-nevermined returns
       proof_not_ready/PENDING_PROOF, poll zpi_get_zkp_status until READY then retry
       pay-with-nevermined; otherwise proceed.
    d. Call settle-via-nevermined (this merchant) with { charge_bundle, external_id }. The
       merchant decrypts the bundle, optionally re-checks the ZPI proof against the attester
       (ZPI_MERCHANT_VERIFY), then verifies and settles the x402 token directly with Nevermined
       and marks the order PAID.
9. To purchase with VGS card flow, call farm-checkout with payment_method='vgs_card'.
    It returns NEEDS_INTENT_PROOF immediately.
    Run chp_save + prove_intent (intent_type='spend'), then call
    checkout-with-credit-card with order_id + external_id + zpi_proof.
10. If checkout-with-credit-card returns READY_FOR_ZPI_PAYMENT, call zpi-zkpay MCP tool
    pay-with-credit-card using zpi_arguments exactly as returned.
11. After zpi-zkpay payment succeeds, call confirm-payment with
    order_id + payment_confirmed=true (+ zpi_response when available).
12. Available categories: dairy, meat, poultry, produce.
13. All prices are in USD.
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
                        "external_id": r.external_id,
                        "proof_id": r.public_proof_id(),
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

#[cfg(test)]
mod nvm_zpi_tests {
    use super::*;

    /// Build a valid SP1 public_values hex fixture matching `parse_committed_public_values`.
    fn build_public_values_fixture() -> String {
        let mut bytes = vec![0u8; 32];
        bytes.extend(bincode::serialize(&3u32).unwrap());
        bytes.extend(bincode::serialize(&"ext-abc".to_string()).unwrap());
        bytes.extend(
            bincode::serialize(&vec![
                ("total_amount".to_string(), [7u8; 32]),
                ("currency".to_string(), [9u8; 32]),
            ])
            .unwrap(),
        );
        hex::encode(bytes)
    }

    #[test]
    fn test_compute_field_commitment_known_vector() {
        // Independently computed: printf 'total_amount:1000:ext-123:salthex' | shasum -a 256
        let expected = "dd295f016f69bf67d2ec9241905dc97d00c4a8097c8765ec859a396128bd115c";
        assert_eq!(
            compute_field_commitment("total_amount", "1000", "ext-123", "salthex"),
            expected
        );
    }

    #[test]
    fn test_compute_field_commitment_changes_with_each_input() {
        let baseline =
            compute_field_commitment("total_amount", "1000", "ext-123", "salthex");
        assert_ne!(
            compute_field_commitment("other_field", "1000", "ext-123", "salthex"),
            baseline
        );
        assert_ne!(
            compute_field_commitment("total_amount", "999", "ext-123", "salthex"),
            baseline
        );
        assert_ne!(
            compute_field_commitment("total_amount", "1000", "ext-456", "salthex"),
            baseline
        );
        assert_ne!(
            compute_field_commitment("total_amount", "1000", "ext-123", "other-salt"),
            baseline
        );
    }

    #[test]
    fn test_parse_public_values_roundtrip() {
        let hex_fixture = build_public_values_fixture();
        let (external_id, commitments) =
            parse_committed_public_values(&hex_fixture).expect("valid fixture should parse");

        assert_eq!(external_id, "ext-abc");
        assert_eq!(commitments.len(), 2);
        assert_eq!(commitments["total_amount"], "07".repeat(32));
        assert_eq!(commitments["currency"], "09".repeat(32));
    }

    #[test]
    fn test_parse_public_values_accepts_0x_prefix() {
        let hex_fixture = build_public_values_fixture();
        let prefixed = format!("0x{hex_fixture}");
        let (external_id, commitments) =
            parse_committed_public_values(&prefixed).expect("0x-prefixed hex should parse");

        assert_eq!(external_id, "ext-abc");
        assert_eq!(commitments.len(), 2);
    }

    #[test]
    fn test_parse_public_values_rejects_bad_hex() {
        let err = parse_committed_public_values("zzzz").unwrap_err();
        assert!(err.contains("hex decode failed"), "unexpected error: {err}");
    }

    #[test]
    fn test_parse_public_values_rejects_truncated() {
        // 10 bytes (< 32-byte intent_commitment header) encoded as hex.
        let truncated = "00".repeat(10);
        let err = parse_committed_public_values(&truncated).unwrap_err();
        assert!(
            err.contains("reading intent_commitment"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_require_amount_commitment_env() {
        const VAR: &str = "ZPI_REQUIRE_AMOUNT_COMMITMENT";

        let original = std::env::var(VAR).ok();

        std::env::remove_var(VAR);
        assert!(zpi_require_amount_commitment());

        std::env::set_var(VAR, "false");
        assert!(!zpi_require_amount_commitment());

        std::env::set_var(VAR, "0");
        assert!(!zpi_require_amount_commitment());

        std::env::set_var(VAR, "true");
        assert!(zpi_require_amount_commitment());

        std::env::set_var(VAR, "1");
        assert!(zpi_require_amount_commitment());

        std::env::set_var(VAR, "FALSE");
        assert!(!zpi_require_amount_commitment());

        match original {
            Some(v) => std::env::set_var(VAR, v),
            None => std::env::remove_var(VAR),
        }
    }

    #[test]
    fn test_zpi_merchant_verify_enabled_env() {
        const VAR: &str = "ZPI_MERCHANT_VERIFY";

        let original = std::env::var(VAR).ok();

        std::env::remove_var(VAR);
        assert!(zpi_merchant_verify_enabled());

        for disabled in ["false", "FALSE", "0", "off", "OFF", "no", "NO"] {
            std::env::set_var(VAR, disabled);
            assert!(
                !zpi_merchant_verify_enabled(),
                "expected disabled for ZPI_MERCHANT_VERIFY={disabled}"
            );
        }

        for enabled in ["true", "1", "yes", "on", ""] {
            std::env::set_var(VAR, enabled);
            assert!(
                zpi_merchant_verify_enabled(),
                "expected enabled for ZPI_MERCHANT_VERIFY={enabled}"
            );
        }

        std::env::set_var(VAR, "  false  ");
        assert!(!zpi_merchant_verify_enabled());

        match original {
            Some(v) => std::env::set_var(VAR, v),
            None => std::env::remove_var(VAR),
        }
    }

    #[test]
    fn test_proof_binding_error_json_shape() {
        let err = proof_binding_error(
            "currency",
            "USD",
            "Use uppercase ISO 4217",
            "proof currency commitment mismatch",
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&err).expect("proof_binding_error should emit JSON");
        assert_eq!(parsed["status"], "PROOF_BINDING_FAILED");
        assert_eq!(parsed["field"], "currency");
        assert_eq!(parsed["merchant_canonical"], "USD");
        assert_eq!(parsed["hint"], "Use uppercase ISO 4217");
        assert!(parsed["message"]
            .as_str()
            .unwrap_or("")
            .contains("mismatch"));
    }
}

#[cfg(test)]
mod nevermined_unit_tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn test_extract_plan_id_from_x402_token() {
        let envelope = json!({
            "x402Version": 1,
            "accepted": {
                "scheme": "visa",
                "network": "sandbox",
                "planId": "plan-abc-123",
                "extra": {}
            },
            "payload": { "token": "inner-jwt" }
        });
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&envelope).unwrap());

        assert_eq!(
            extract_plan_id_from_x402_token(&token).as_deref(),
            Some("plan-abc-123")
        );
        assert_eq!(extract_plan_id_from_x402_token("not-base64!!!"), None);
    }

    #[test]
    fn test_resolve_merchant_bearer_env_priority() {
        const MERCHANT_VAR: &str = "NVM_MERCHANT_API_KEY";
        const LEGACY_VAR: &str = "NVM_API_KEY";

        let original_merchant = std::env::var(MERCHANT_VAR).ok();
        let original_legacy = std::env::var(LEGACY_VAR).ok();

        std::env::remove_var(MERCHANT_VAR);
        std::env::remove_var(LEGACY_VAR);
        assert_eq!(resolve_merchant_bearer("x402-token"), "x402-token");

        std::env::set_var(LEGACY_VAR, "legacy-key");
        assert_eq!(resolve_merchant_bearer("x402-token"), "legacy-key");

        std::env::set_var(MERCHANT_VAR, "merchant-key");
        assert_eq!(resolve_merchant_bearer("x402-token"), "merchant-key");

        std::env::set_var(MERCHANT_VAR, "   ");
        assert_eq!(resolve_merchant_bearer("x402-token"), "legacy-key");

        match original_merchant {
            Some(v) => std::env::set_var(MERCHANT_VAR, v),
            None => std::env::remove_var(MERCHANT_VAR),
        }
        match original_legacy {
            Some(v) => std::env::set_var(LEGACY_VAR, v),
            None => std::env::remove_var(LEGACY_VAR),
        }
    }

    #[test]
    fn test_redact_body_text_masks_sensitive_json_fields() {
        let body = r#"{"token":"super-secret-token-value","status":"ok"}"#;
        let redacted = redact_body_text(body);
        assert!(!redacted.contains("super-secret-token-value"));
        assert!(redacted.contains("ok"));
    }

    #[test]
    fn test_redact_body_text_non_json() {
        assert_eq!(redact_body_text("short"), "<redacted>");
        let long = "x".repeat(150);
        let redacted = redact_body_text(&long);
        assert!(redacted.contains("…<redacted>"));
        assert!(!redacted.contains(&"x".repeat(150)));
    }
}

#[cfg(test)]
mod attester_http_tests {
    use super::*;
    use axum::{
        extract::{Path, Query},
        routing::get,
        Router,
    };
    use std::sync::{Arc, Mutex};

    /// Bind an ephemeral mock attester and return its base URL (`http://127.0.0.1:PORT`).
    async fn spawn_mock_attester(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn commitment_hex_to_bytes(hex_str: &str) -> [u8; 32] {
        let bytes = hex::decode(hex_str).unwrap();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        arr
    }

    /// Build SP1 public_values hex matching `parse_committed_public_values`.
    fn build_public_values_hex(external_id: &str, field_pairs: Vec<(String, [u8; 32])>) -> String {
        let mut bytes = vec![0u8; 32];
        bytes.extend(bincode::serialize(&3u32).unwrap());
        bytes.extend(bincode::serialize(&external_id.to_string()).unwrap());
        bytes.extend(bincode::serialize(&field_pairs).unwrap());
        hex::encode(bytes)
    }

    fn build_bound_public_values(
        external_id: &str,
        amount_cents: u64,
        salt: &str,
    ) -> (String, String) {
        let program_id = "sha256:deadbeef".to_string();
        let amount_hex =
            compute_field_commitment("total_amount", &amount_cents.to_string(), external_id, salt);
        let amount_bytes = commitment_hex_to_bytes(&amount_hex);
        let currency_hex = compute_field_commitment("currency", "USD", external_id, salt);
        let currency_bytes = commitment_hex_to_bytes(&currency_hex);
        let public_values_hex = build_public_values_hex(
            external_id,
            vec![
                ("total_amount".to_string(), amount_bytes),
                ("currency".to_string(), currency_bytes),
            ],
        );
        (public_values_hex, program_id)
    }

    #[derive(Deserialize)]
    struct DeriveSaltQuery {
        external_id: String,
    }

    #[tokio::test]
    async fn test_verify_attester_proof_by_id_ok() {
        let app = Router::new().route(
            "/proofs/:id/verify",
            get(|Path(id): Path<String>| async move {
                Json(json!({
                    "success": true,
                    "data": { "proof": { "session_id": format!("sess-{id}") } }
                }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        match verify_attester_proof_by_id(&client, &base, "proof-1").await {
            AttesterVerifyOutcome::Ok { session_id } => {
                assert_eq!(session_id.as_deref(), Some("sess-proof-1"));
            }
            _other => panic!("expected Ok, got unexpected AttesterVerifyOutcome variant"),
        }
    }

    #[tokio::test]
    async fn test_verify_attester_proof_by_id_not_found_404() {
        let app = Router::new().route(
            "/proofs/:id/verify",
            get(|_: Path<String>| async { StatusCode::NOT_FOUND }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        assert!(matches!(
            verify_attester_proof_by_id(&client, &base, "missing").await,
            AttesterVerifyOutcome::NotFound
        ));
    }

    #[tokio::test]
    async fn test_verify_attester_proof_by_id_failed_success_false() {
        let app = Router::new().route(
            "/proofs/:id/verify",
            get(|_: Path<String>| async move {
                Json(json!({ "success": false, "error": "proof stale or unverified" }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        match verify_attester_proof_by_id(&client, &base, "stale").await {
            AttesterVerifyOutcome::Failed(msg) => {
                assert!(msg.contains("proof verification failed"));
                assert!(msg.contains("proof stale or unverified"));
            }
            _ => panic!("expected Failed AttesterVerifyOutcome variant"),
        }
    }

    #[tokio::test]
    async fn test_verify_attester_proof_by_id_failed_http_500() {
        let app = Router::new().route(
            "/proofs/:id/verify",
            get(|_: Path<String>| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        match verify_attester_proof_by_id(&client, &base, "boom").await {
            AttesterVerifyOutcome::Failed(msg) => {
                assert!(msg.contains("attester returned HTTP 500"));
            }
            _ => panic!("expected Failed AttesterVerifyOutcome variant"),
        }
    }

    #[tokio::test]
    async fn test_resolve_attester_proof_id_by_external_id_ok() {
        let app = Router::new().route(
            "/proofs/session/:external_id",
            get(|Path(external_id): Path<String>| async move {
                Json(json!({
                    "proofs": [
                        {
                            "proof_id": "older-id",
                            "tool_name": "attest",
                            "timestamp": 1
                        },
                        {
                            "proof_id": format!("resolved-{external_id}"),
                            "tool_name": "attest",
                            "timestamp": 99
                        }
                    ]
                }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let resolved =
            resolve_attester_proof_id_by_external_id(&client, &base, "ext-abc")
                .await
                .expect("should resolve proof id");
        assert_eq!(resolved, "resolved-ext-abc");
    }

    #[tokio::test]
    async fn test_resolve_attester_proof_id_by_external_id_empty_proofs() {
        let app = Router::new().route(
            "/proofs/session/:external_id",
            get(|_: Path<String>| async move { Json(json!({ "proofs": [] })) }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = resolve_attester_proof_id_by_external_id(&client, &base, "ext-empty")
            .await
            .unwrap_err();
        assert!(err.contains("no proofs found at attester"));
    }

    #[tokio::test]
    async fn test_fetch_proof_material_by_id_ok() {
        let app = Router::new().route(
            "/proofs/:id",
            get(|Path(_id): Path<String>| async move {
                Json(json!({
                    "success": true,
                    "data": {
                        "proof": {
                            "proof": {
                                "public_values": "abc123",
                                "vk_hash": "vk-deadbeef",
                                "program_id": "sha256:cafebabe",
                                "field_commitments": [
                                    { "field": "total_amount", "commitment": "ff" }
                                ]
                            }
                        }
                    }
                }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let material = fetch_proof_material_by_id(&client, &base, "proof-42")
            .await
            .expect("fetch should succeed")
            .expect("proof should exist");

        assert_eq!(material.public_values_hex, "abc123");
        assert_eq!(material.vk_hash.as_deref(), Some("vk-deadbeef"));
        assert_eq!(material.program_id.as_deref(), Some("sha256:cafebabe"));
        assert!(material.field_commitments_meta.is_some());
    }

    #[tokio::test]
    async fn test_fetch_proof_material_by_id_not_found() {
        let app = Router::new().route(
            "/proofs/:id",
            get(|_: Path<String>| async { StatusCode::NOT_FOUND }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let result = fetch_proof_material_by_id(&client, &base, "missing")
            .await
            .expect("transport ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_derive_salt_from_attester_ok_encodes_program_id() {
        let encoded_hit = Arc::new(Mutex::new(false));
        let decoded_hit = Arc::new(Mutex::new(false));
        let encoded_hit_clone = encoded_hit.clone();
        let decoded_hit_clone = decoded_hit.clone();
        let captured_external = Arc::new(Mutex::new(String::new()));
        let captured_external_clone = captured_external.clone();

        let app = Router::new()
            .route(
                "/programs/sha256%3Adeadbeef/derive-salt",
                get({
                    let encoded_hit = encoded_hit_clone;
                    let captured_external = captured_external_clone;
                    move |Query(q): Query<DeriveSaltQuery>| {
                        let encoded_hit = encoded_hit.clone();
                        let captured_external = captured_external.clone();
                        async move {
                            *encoded_hit.lock().unwrap() = true;
                            *captured_external.lock().unwrap() = q.external_id;
                            Json(json!({ "derived_salt": "cafebabe" }))
                        }
                    }
                }),
            )
            .route(
                "/programs/sha256:deadbeef/derive-salt",
                get({
                    let decoded_hit = decoded_hit_clone;
                    move || {
                        let decoded_hit = decoded_hit.clone();
                        async move {
                            *decoded_hit.lock().unwrap() = true;
                            Json(json!({ "derived_salt": "wrong-route" }))
                        }
                    }
                }),
            );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let salt = derive_salt_from_attester(
            &client,
            &base,
            "sha256:deadbeef",
            "ext-derive",
        )
        .await
        .expect("derive-salt should succeed");
        assert_eq!(salt, "cafebabe");
        assert!(*encoded_hit.lock().unwrap(), "expected percent-encoded program_id route");
        assert!(
            !*decoded_hit.lock().unwrap(),
            "must not hit decoded-colon route (would break content-addressed ids)"
        );
        assert_eq!(captured_external.lock().unwrap().as_str(), "ext-derive");
    }

    #[tokio::test]
    async fn test_derive_salt_from_attester_http_error() {
        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async {
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = derive_salt_from_attester(&client, &base, "sha256:x", "ext-1")
            .await
            .unwrap_err();
        assert!(err.contains("derive-salt returned HTTP 500"));
    }

    #[tokio::test]
    async fn test_derive_salt_from_attester_missing_field() {
        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async move {
                Json(json!({ "unexpected": true }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = derive_salt_from_attester(&client, &base, "sha256:x", "ext-1")
            .await
            .unwrap_err();
        assert!(err.contains("derive-salt response missing `derived_salt`"));
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_happy_path() {
        const EXTERNAL_ID: &str = "ext-bind-ok";
        const AMOUNT_CENTS: u64 = 2500;
        const SALT: &str = "salthex123";

        let (public_values_hex, program_id) =
            build_bound_public_values(EXTERNAL_ID, AMOUNT_CENTS, SALT);
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some(program_id),
            field_commitments_meta: None,
        };

        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(
                |_: Path<String>, Query(q): Query<DeriveSaltQuery>| async move {
                    assert_eq!(q.external_id, EXTERNAL_ID);
                    Json(json!({ "derived_salt": SALT }))
                },
            ),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        run_zero_trust_proof_checks(&client, &base, &material, EXTERNAL_ID, AMOUNT_CENTS, "USD")
            .await
            .expect("happy path should pass");
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_amount_tamper() {
        const EXTERNAL_ID: &str = "ext-tamper";
        const AMOUNT_CENTS: u64 = 500;
        const SALT: &str = "good-salt";

        let (public_values_hex, program_id) =
            build_bound_public_values(EXTERNAL_ID, AMOUNT_CENTS + 1, SALT);

        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some(program_id),
            field_commitments_meta: None,
        };

        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async move {
                Json(json!({ "derived_salt": SALT }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = run_zero_trust_proof_checks(
            &client,
            &base,
            &material,
            EXTERNAL_ID,
            AMOUNT_CENTS,
            "USD",
        )
        .await
        .unwrap_err();
        assert!(err.contains("amount binding FAILED"));
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_external_id_mismatch() {
        const COMMITTED_ID: &str = "ext-committed";
        const SETTLING_ID: &str = "ext-settling";
        const AMOUNT_CENTS: u64 = 100;
        const SALT: &str = "salt";

        let (public_values_hex, program_id) =
            build_bound_public_values(COMMITTED_ID, AMOUNT_CENTS, SALT);
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some(program_id),
            field_commitments_meta: None,
        };

        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async move {
                Json(json!({ "derived_salt": SALT }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = run_zero_trust_proof_checks(
            &client,
            &base,
            &material,
            SETTLING_ID,
            AMOUNT_CENTS,
            "USD",
        )
        .await
        .unwrap_err();
        assert!(err.contains("external_id binding FAILED"));
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_missing_total_amount_required() {
        static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        const VAR: &str = "ZPI_REQUIRE_AMOUNT_COMMITMENT";
        let original = std::env::var(VAR).ok();
        std::env::remove_var(VAR);
        assert!(zpi_require_amount_commitment());

        const EXTERNAL_ID: &str = "ext-no-amount";

        let public_values_hex =
            build_public_values_hex(EXTERNAL_ID, vec![("currency".to_string(), [1u8; 32])]);
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some("sha256:abc".into()),
            field_commitments_meta: None,
        };

        let base = spawn_mock_attester(Router::new()).await;
        let client = reqwest::Client::new();

        let err = run_zero_trust_proof_checks(&client, &base, &material, EXTERNAL_ID, 999, "USD")
            .await
            .unwrap_err();
        assert!(err.contains("does not commit a `total_amount` field"));

        match original {
            Some(v) => std::env::set_var(VAR, v),
            None => std::env::remove_var(VAR),
        }
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_missing_currency_required() {
        const EXTERNAL_ID: &str = "ext-no-currency";
        const AMOUNT_CENTS: u64 = 999;
        const SALT: &str = "salt";
        let amount_hex =
            compute_field_commitment("total_amount", &AMOUNT_CENTS.to_string(), EXTERNAL_ID, SALT);
        let public_values_hex = build_public_values_hex(
            EXTERNAL_ID,
            vec![(
                "total_amount".to_string(),
                commitment_hex_to_bytes(&amount_hex),
            )],
        );
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some("sha256:abc".into()),
            field_commitments_meta: None,
        };
        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async move {
                Json(json!({ "derived_salt": SALT }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = run_zero_trust_proof_checks(
            &client,
            &base,
            &material,
            EXTERNAL_ID,
            AMOUNT_CENTS,
            "USD",
        )
        .await
        .unwrap_err();
        assert!(err.contains("does not commit a `currency` field"));
    }

    #[test]
    fn test_canonicalize_merchant_currency_lowercase_request() {
        assert_eq!(canonicalize_merchant_currency(None), "USD");
        assert_eq!(canonicalize_merchant_currency(Some("usd")), "USD");
        assert_eq!(canonicalize_merchant_currency(Some("  eur ")), "EUR");
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_lowercase_request_currency_accepted() {
        const EXTERNAL_ID: &str = "ext-currency-req";
        const AMOUNT_CENTS: u64 = 1200;
        const SALT: &str = "salt-req-usd";

        let (public_values_hex, program_id) =
            build_bound_public_values(EXTERNAL_ID, AMOUNT_CENTS, SALT);
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some(program_id),
            field_commitments_meta: None,
        };

        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(
                |_: Path<String>, Query(q): Query<DeriveSaltQuery>| async move {
                    assert_eq!(q.external_id, EXTERNAL_ID);
                    Json(json!({ "derived_salt": SALT }))
                },
            ),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        run_zero_trust_proof_checks(
            &client,
            &base,
            &material,
            EXTERNAL_ID,
            AMOUNT_CENTS,
            "usd",
        )
        .await
        .expect("lowercase request currency should canonicalize to USD and match proof");
    }

    fn build_bound_public_values_with_currency(
        external_id: &str,
        amount_cents: u64,
        currency: &str,
        salt: &str,
    ) -> (String, String) {
        let program_id = "sha256:deadbeef".to_string();
        let amount_hex =
            compute_field_commitment("total_amount", &amount_cents.to_string(), external_id, salt);
        let amount_bytes = commitment_hex_to_bytes(&amount_hex);
        let currency_hex = compute_field_commitment("currency", currency, external_id, salt);
        let currency_bytes = commitment_hex_to_bytes(&currency_hex);
        let public_values_hex = build_public_values_hex(
            external_id,
            vec![
                ("total_amount".to_string(), amount_bytes),
                ("currency".to_string(), currency_bytes),
            ],
        );
        (public_values_hex, program_id)
    }

    #[tokio::test]
    async fn test_run_zero_trust_proof_checks_lowercase_proof_currency_rejected() {
        const EXTERNAL_ID: &str = "ext-currency-proof";
        const AMOUNT_CENTS: u64 = 800;
        const SALT: &str = "salt-proof-usd";

        let (public_values_hex, program_id) = build_bound_public_values_with_currency(
            EXTERNAL_ID,
            AMOUNT_CENTS,
            "usd",
            SALT,
        );
        let material = ProofMaterial {
            public_values_hex,
            vk_hash: None,
            program_id: Some(program_id),
            field_commitments_meta: None,
        };

        let app = Router::new().route(
            "/programs/:program_id/derive-salt",
            get(|_: Path<String>, _: Query<DeriveSaltQuery>| async move {
                Json(json!({ "derived_salt": SALT }))
            }),
        );
        let base = spawn_mock_attester(app).await;
        let client = reqwest::Client::new();

        let err = run_zero_trust_proof_checks(
            &client,
            &base,
            &material,
            EXTERNAL_ID,
            AMOUNT_CENTS,
            "USD",
        )
        .await
        .unwrap_err();

        let parsed: serde_json::Value =
            serde_json::from_str(&err).expect("currency binding error should be structured JSON");
        assert_eq!(parsed["status"], "PROOF_BINDING_FAILED");
        assert_eq!(parsed["field"], "currency");
        assert_eq!(parsed["merchant_canonical"], "USD");
        assert!(parsed["hint"]
            .as_str()
            .unwrap_or("")
            .contains("uppercase"));
        assert!(parsed["message"]
            .as_str()
            .unwrap_or("")
            .contains("currency commitment"));
    }
}
