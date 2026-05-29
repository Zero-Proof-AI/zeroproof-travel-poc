/// Stripe Checkout integration for Green Acres Farm.
///
/// Endpoints:
///   POST /api/stripe/create-checkout-session  — create a Stripe-hosted checkout session
///   GET  /stripe/success                       — redirect target after successful payment
///   GET  /stripe/cancel                        — redirect target after cancelled payment
///   POST /api/stripe/webhook                   — receive Stripe webhook events
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use farm_core::types::{CartItem, Order, OrderStatus, PaymentMethod};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use super::db::SharedMerchantDb;
use super::state::SharedFarmState;

// ── Request / Response types ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateCheckoutRequest {
    pub items: Vec<CheckoutLineItem>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CheckoutLineItem {
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

#[derive(Debug, Deserialize)]
pub struct StripeSuccessQuery {
    pub session_id: Option<String>,
}

// ── Handlers ────────────────────────────────────────────────────

/// POST /api/stripe/create-checkout-session
///
/// Creates a Stripe-hosted Checkout Session for the given cart items,
/// persists a pending order, and returns the Stripe redirect URL.
pub async fn handle_create_checkout_session(
    State(farm_state): State<SharedFarmState>,
    State(merchant_db): State<SharedMerchantDb>,
    Json(req): Json<CreateCheckoutRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let secret_key = std::env::var("STRIPE_SECRET_KEY").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "STRIPE_SECRET_KEY not configured"})),
        )
    })?;

    if req.items.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Cart is empty"})),
        ));
    }

    // Validate quantities and prices
    for item in &req.items {
        if item.quantity == 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Item quantity must be at least 1"})),
            ));
        }
        if item.unit_price_cents == 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Item price must be greater than zero"})),
            ));
        }
    }

    let base_url = std::env::var("BASE_URL").or_else(|_| std::env::var("SERVER_BASE_URL")).unwrap_or_else(|_| {
        let port = std::env::var("PORT").unwrap_or_else(|_| "8001".to_string());
        format!("http://localhost:{}", port)
    });

    // {CHECKOUT_SESSION_ID} is a Stripe template literal — it is replaced by Stripe at redirect time
    let success_url = format!("{}/stripe/success?session_id={{CHECKOUT_SESSION_ID}}", base_url);
    let cancel_url = format!("{}/stripe/cancel", base_url);

    // Build form-encoded params for Stripe Checkout Session API
    let mut params: Vec<(String, String)> = vec![
        ("mode".to_string(), "payment".to_string()),
        ("success_url".to_string(), success_url),
        ("cancel_url".to_string(), cancel_url),
        ("payment_method_types[]".to_string(), "card".to_string()),
    ];

    for (i, item) in req.items.iter().enumerate() {
        params.push((
            format!("line_items[{}][price_data][currency]", i),
            "usd".to_string(),
        ));
        params.push((
            format!("line_items[{}][price_data][product_data][name]", i),
            item.name.clone(),
        ));
        params.push((
            format!("line_items[{}][price_data][unit_amount]", i),
            item.unit_price_cents.to_string(),
        ));
        params.push((
            format!("line_items[{}][quantity]", i),
            item.quantity.to_string(),
        ));
    }

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .basic_auth(&secret_key, Some(""))
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("[STRIPE] Checkout session request failed: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": format!("Stripe request failed: {}", e)})),
            )
        })?;

    let http_status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);

    if !http_status.is_success() {
        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Stripe error");
        tracing::error!("[STRIPE] Checkout session creation failed ({}): {}", http_status, msg);
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": msg})),
        ));
    }

    let stripe_session_id = body["id"].as_str().unwrap_or("").to_string();
    let url = body["url"].as_str().unwrap_or("").to_string();

    if stripe_session_id.is_empty() || url.is_empty() {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": "Invalid response from Stripe"})),
        ));
    }

    // Create a pending order backed by the Stripe session_id
    let order_id = format!(
        "farm-stripe-{}",
        &Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let total_cents: u64 = req
        .items
        .iter()
        .map(|i| i.unit_price_cents * i.quantity as u64)
        .sum();

    let order = Order {
        order_id: order_id.clone(),
        // Use the Stripe session_id so we can look it up on webhook/success
        session_id: stripe_session_id.clone(),
        items: req
            .items
            .iter()
            .map(|i| CartItem {
                product_id: i.name.to_lowercase().replace(' ', "-"),
                quantity: i.quantity,
                unit_price_cents: i.unit_price_cents,
            })
            .collect(),
        total_cents,
        status: OrderStatus::PendingPayment,
        payment_method: PaymentMethod::Stripe,
    };

    // Persist to SQLite
    if let Err(e) = merchant_db.insert_order(&order) {
        tracing::error!("[STRIPE] Failed to persist order {}: {}", order_id, e);
    }

    // Also keep in-memory state
    {
        let mut state = farm_state.write().await;
        state.orders.insert(order_id.clone(), order);
    }

    tracing::info!(
        "[STRIPE] Checkout session created: order_id={}, stripe_session_id={}",
        order_id,
        stripe_session_id
    );

    Ok(Json(serde_json::json!({
        "url": url,
        "session_id": stripe_session_id,
        "order_id": order_id,
    })))
}

/// GET /stripe/success?session_id=cs_...
///
/// Stripe redirects the customer here after a successful payment.
/// Verifies the session with Stripe and marks the order as paid.
pub async fn handle_stripe_success(
    State(farm_state): State<SharedFarmState>,
    State(merchant_db): State<SharedMerchantDb>,
    Query(query): Query<StripeSuccessQuery>,
) -> Html<String> {
    let session_id = query.session_id.unwrap_or_default();

    let order_id = if !session_id.is_empty() {
        // Verify payment with Stripe and update order
        if let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") {
            match fetch_stripe_session(&secret_key, &session_id).await {
                Ok(session) => {
                    let payment_status = session["payment_status"].as_str().unwrap_or("");
                    if payment_status == "paid" {
                        // Update in DB
                        if let Err(e) = merchant_db.update_order_status(
                            // Find by session_id
                            &merchant_db
                                .get_order_by_session_id(&session_id)
                                .map(|r| r.order_id)
                                .unwrap_or_default(),
                            &OrderStatus::Paid,
                            Some(&session_id),
                            Some("stripe"),
                        ) {
                            tracing::error!("[STRIPE] Failed to update order status: {}", e);
                        }

                        // Update in-memory state
                        let mut state = farm_state.write().await;
                        for order in state.orders.values_mut() {
                            if order.session_id == session_id {
                                order.status = OrderStatus::Paid;
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[STRIPE] Success redirect for session {} but payment_status={}",
                            session_id,
                            payment_status
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "[STRIPE] Failed to verify session {} with Stripe: {}",
                        session_id,
                        e
                    );
                }
            }
        }

        // Look up order_id for display
        merchant_db
            .get_order_by_session_id(&session_id)
            .map(|r| r.order_id)
    } else {
        None
    };

    let order_display = order_id
        .as_deref()
        .map(|oid| format!("<div class=\"order-id\">Order ID: {}</div>", html_escape(oid)))
        .unwrap_or_default();

    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Payment Successful — Green Acres Farm</title>
  <style>
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: #f0faf4;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
      margin: 0;
    }}
    .card {{
      background: white;
      border-radius: 16px;
      padding: 3rem 2.5rem;
      text-align: center;
      box-shadow: 0 4px 20px rgba(0,0,0,0.1);
      max-width: 420px;
      width: 90vw;
    }}
    .icon {{ font-size: 4rem; margin-bottom: 1rem; }}
    h1 {{ color: #2d6a4f; margin-bottom: 0.5rem; font-size: 1.8rem; }}
    p {{ color: #6c757d; margin-bottom: 1.5rem; }}
    .order-id {{
      background: #f0faf4;
      padding: 0.5rem 1rem;
      border-radius: 8px;
      font-family: monospace;
      font-size: 0.85rem;
      color: #2d6a4f;
      margin-bottom: 1.5rem;
    }}
    a {{
      background: #2d6a4f;
      color: white;
      text-decoration: none;
      padding: 0.7rem 1.5rem;
      border-radius: 8px;
      font-weight: 600;
      display: inline-block;
    }}
    a:hover {{ background: #52b788; }}
  </style>
</head>
<body>
  <div class="card">
    <div class="icon">✅</div>
    <h1>Payment Successful!</h1>
    <p>Thank you for your purchase from Green Acres Farm.<br>Your order has been received.</p>
    {order_display}
    <a href="/">Back to Shop</a>
  </div>
</body>
</html>"#,
        order_display = order_display
    ))
}

/// GET /stripe/cancel
///
/// Stripe redirects here when the customer cancels the payment.
pub async fn handle_stripe_cancel() -> Html<String> {
    Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Payment Cancelled — Green Acres Farm</title>
  <style>
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: #f0faf4;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
      margin: 0;
    }
    .card {
      background: white;
      border-radius: 16px;
      padding: 3rem 2.5rem;
      text-align: center;
      box-shadow: 0 4px 20px rgba(0,0,0,0.1);
      max-width: 420px;
      width: 90vw;
    }
    .icon { font-size: 4rem; margin-bottom: 1rem; }
    h1 { color: #343a40; margin-bottom: 0.5rem; font-size: 1.8rem; }
    p { color: #6c757d; margin-bottom: 1.5rem; }
    a {
      background: #2d6a4f;
      color: white;
      text-decoration: none;
      padding: 0.7rem 1.5rem;
      border-radius: 8px;
      font-weight: 600;
      display: inline-block;
    }
    a:hover { background: #52b788; }
  </style>
</head>
<body>
  <div class="card">
    <div class="icon">🛒</div>
    <h1>Payment Cancelled</h1>
    <p>No worries — your cart items are still waiting for you.</p>
    <a href="/">Return to Shop</a>
  </div>
</body>
</html>"#
            .to_string(),
    )
}

/// POST /api/stripe/webhook
///
/// Receives Stripe webhook events. Verifies the `Stripe-Signature` header
/// using HMAC-SHA256 before processing. Handles `checkout.session.completed`
/// to reliably mark orders as paid.
pub async fn handle_stripe_webhook(
    State(farm_state): State<SharedFarmState>,
    State(merchant_db): State<SharedMerchantDb>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let webhook_secret = std::env::var("STRIPE_WEBHOOK_SECRET").map_err(|_| {
        tracing::warn!("[STRIPE WEBHOOK] STRIPE_WEBHOOK_SECRET not configured");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Webhook secret not configured"})),
        )
    })?;

    let sig_header = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing Stripe-Signature header"})),
            )
        })?;

    verify_stripe_signature(sig_header, &body, &webhook_secret).map_err(|e| {
        tracing::warn!("[STRIPE WEBHOOK] Signature verification failed: {}", e);
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid signature: {}", e)})),
        )
    })?;

    let event: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid JSON: {}", e)})),
        )
    })?;

    let event_type = event["type"].as_str().unwrap_or("");
    tracing::info!("[STRIPE WEBHOOK] Received event: {}", event_type);

    match event_type {
        "checkout.session.completed" => {
            let session_id = event["data"]["object"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let payment_status = event["data"]["object"]["payment_status"]
                .as_str()
                .unwrap_or("");

            tracing::info!(
                "[STRIPE WEBHOOK] checkout.session.completed: session={}, payment_status={}",
                session_id,
                payment_status
            );

            if payment_status == "paid" {
                // Update DB
                if let Some(row) = merchant_db.get_order_by_session_id(&session_id) {
                    if let Err(e) = merchant_db.update_order_status(
                        &row.order_id,
                        &OrderStatus::Paid,
                        Some(&session_id),
                        Some("stripe"),
                    ) {
                        tracing::error!(
                            "[STRIPE WEBHOOK] Failed to update order {}: {}",
                            row.order_id,
                            e
                        );
                    } else {
                        tracing::info!(
                            "[STRIPE WEBHOOK] Order {} marked as paid",
                            row.order_id
                        );
                    }
                }

                // Update in-memory state
                let mut state = farm_state.write().await;
                for order in state.orders.values_mut() {
                    if order.session_id == session_id {
                        order.status = OrderStatus::Paid;
                    }
                }
            }
        }
        "checkout.session.expired" => {
            let session_id = event["data"]["object"]["id"]
                .as_str()
                .unwrap_or("")
                .to_string();
            tracing::info!("[STRIPE WEBHOOK] Session expired: {}", session_id);
            // Optionally cancel the order
            if let Some(row) = merchant_db.get_order_by_session_id(&session_id) {
                let _ = merchant_db.update_order_status(
                    &row.order_id,
                    &OrderStatus::Cancelled,
                    None,
                    None,
                );
            }
        }
        _ => {
            tracing::debug!("[STRIPE WEBHOOK] Unhandled event type: {}", event_type);
        }
    }

    Ok(Json(serde_json::json!({"received": true})))
}

// ── Stripe API helpers ───────────────────────────────────────────

/// Fetch a Checkout Session from the Stripe API to verify its payment status.
async fn fetch_stripe_session(
    secret_key: &str,
    session_id: &str,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!(
            "https://api.stripe.com/v1/checkout/sessions/{}",
            session_id
        ))
        .basic_auth(secret_key, Some(""))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Stripe API returned {}", response.status()));
    }

    response
        .json()
        .await
        .map_err(|e| format!("Parse failed: {}", e))
}

// ── Webhook signature verification ──────────────────────────────

/// Verify a Stripe webhook signature.
///
/// Stripe sends the header: `t=<unix_timestamp>,v1=<hmac_hex>[,v1=<hmac_hex>]`
/// The signed payload is `{timestamp}.{raw_body}`.
fn verify_stripe_signature(
    sig_header: &str,
    payload: &[u8],
    secret: &str,
) -> Result<(), String> {
    let mut timestamp: Option<&str> = None;
    let mut signatures: Vec<&str> = Vec::new();

    for part in sig_header.split(',') {
        if let Some(ts) = part.strip_prefix("t=") {
            timestamp = Some(ts);
        } else if let Some(sig) = part.strip_prefix("v1=") {
            signatures.push(sig);
        }
    }

    let timestamp = timestamp.ok_or_else(|| "Missing timestamp in Stripe-Signature".to_string())?;

    if signatures.is_empty() {
        return Err("No v1 signatures found in Stripe-Signature".to_string());
    }

    // Signed payload = "{timestamp}.{raw_body}"
    let mut signed_payload = timestamp.as_bytes().to_vec();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(payload);

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("Invalid HMAC key: {}", e))?;
    mac.update(&signed_payload);
    let expected = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison to prevent timing attacks
    let matches = signatures
        .iter()
        .any(|sig| constant_time_eq(sig.as_bytes(), expected.as_bytes()));

    if matches {
        Ok(())
    } else {
        Err("Signature does not match".to_string())
    }
}

// ── Agentic PSP: Stripe network-token charge ────────────────────

/// Charge a VGS network token (DPAN) via Stripe PaymentIntents API.
///
/// Called by `confirm-vgs-credit-card-payment` after the JWE charge_bundle
/// is decrypted. The DPAN and one-time cryptogram from VGS CMP are forwarded
/// to Stripe as a confirmed PaymentIntent.
///
/// Returns the Stripe PaymentIntent ID (`pi_...`) on success.
pub async fn charge_with_network_token(
    secret_key: &str,
    dpan: &str,
    exp_month: u32,
    exp_year: u32,
    _cryptogram: &str,      // TAVV from VGS CMP; kept for future audit use
    _cryptogram_type: &str, // reserved for future network-token type routing
    amount_cents: u64,
    currency_numeric: &str, // ISO 4217 numeric, e.g. "840" for USD
    external_id: &str,
    merchant_id: &str,
    cavv: Option<&str>,     // 3DS CAVV from PAAY (None when bypassed in test mode)
    eci: Option<&str>,      // 3DS ECI from PAAY
) -> Result<String, String> {
    let currency = numeric_to_alpha_currency(currency_numeric);

    // STRIPE_DPAN_MOCK_SUCCESS=true → both Stripe requests still fire and are logged;
    // if either fails, the error is caught and a synthetic pi_mock_* ID is returned.
    // Use during PoC / sandbox runs where Stripe test mode rejects non-whitelisted DPANs.
    let mock_success = std::env::var("STRIPE_DPAN_MOCK_SUCCESS")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    if mock_success {
        tracing::warn!(
            "[STRIPE] DPAN mock-success enabled — Stripe requests will fire but errors \
             will be swallowed (amount={} {}, merchant={}, dpan_last4={})",
            amount_cents,
            currency,
            merchant_id,
            &dpan[dpan.len().saturating_sub(4)..],
        );
    }

    let return_url = std::env::var("STRIPE_RETURN_URL")
        .unwrap_or_else(|_| "https://example.com/payment-complete".to_string());

    let client = reqwest::Client::new();

    // ── Step 1: Create a PaymentMethod with the DPAN ─────────────────────────
    let pm_params: Vec<(String, String)> = vec![
        ("type".to_string(), "card".to_string()),
        ("card[number]".to_string(), dpan.to_string()),
        ("card[exp_month]".to_string(), exp_month.to_string()),
        ("card[exp_year]".to_string(), exp_year.to_string()),
    ];

    tracing::info!(
        "[STRIPE] POST /v1/payment_methods — type=card card[number]={}***{} \
         card[exp_month]={} card[exp_year]={} idempotency-key=zpi-pm-{}",
        &dpan[..dpan.len().min(6)],
        &dpan[dpan.len().saturating_sub(4)..],
        exp_month,
        exp_year,
        external_id,
    );

    let pm_response = client
        .post("https://api.stripe.com/v1/payment_methods")
        .basic_auth(secret_key, Some(""))
        .header("Stripe-Version", "2024-06-20")
        .header("Idempotency-Key", format!("zpi-pm-{}", external_id))
        .form(&pm_params)
        .send()
        .await
        .map_err(|e| format!("Stripe PaymentMethod request failed: {}", e))?;

    let pm_http_status = pm_response.status();
    let pm_body: serde_json::Value = pm_response
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);

    tracing::info!(
        "[STRIPE] POST /v1/payment_methods — HTTP {} body={}",
        pm_http_status,
        pm_body,
    );

    if !pm_http_status.is_success() {
        let err_msg = pm_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Stripe error");
        if mock_success {
            let mock_id = format!("pi_mock_{}", external_id);
            tracing::warn!(
                "[STRIPE] mock-success: swallowing PM error (HTTP {}: {}) → id={}",
                pm_http_status, err_msg, mock_id
            );
            return Ok(mock_id);
        }
        return Err(format!(
            "Stripe PaymentMethod creation failed (HTTP {}): {}",
            pm_http_status, err_msg
        ));
    }

    let payment_method_id = pm_body["id"]
        .as_str()
        .ok_or_else(|| "Stripe returned no PaymentMethod id".to_string())?
        .to_string();

    tracing::info!("[STRIPE] PaymentMethod created: id={}", payment_method_id);

    // ── Step 2: Create + confirm PaymentIntent using the PM id ───────────────
    let mut pi_params: Vec<(String, String)> = vec![
        ("amount".to_string(), amount_cents.to_string()),
        ("currency".to_string(), currency),
        ("payment_method".to_string(), payment_method_id),
        ("confirm".to_string(), "true".to_string()),
        ("return_url".to_string(), return_url),
        ("metadata[external_id]".to_string(), external_id.to_string()),
        ("metadata[merchant_id]".to_string(), merchant_id.to_string()),
        ("metadata[credential_mode]".to_string(), "cmp_dpan".to_string()),
    ];

    // Include 3DS auth data only when PAAY performed real 3DS authentication.
    // When PAAY is bypassed (sandbox / DPAN-only flow), omit three_d_secure entirely.
    // NOTE: in production with PAAY active, also pass the real dsTransID.
    if let Some(cavv_val) = cavv {
        pi_params.push((
            "payment_method_options[card][three_d_secure][cryptogram]".to_string(),
            cavv_val.to_string(),
        ));
        pi_params.push((
            "payment_method_options[card][three_d_secure][version]".to_string(),
            "2.2.0".to_string(),
        ));
        let eci_value = eci.unwrap_or("05");
        pi_params.push((
            "payment_method_options[card][three_d_secure][electronic_commerce_indicator]"
                .to_string(),
            eci_value.to_string(),
        ));
        // transaction_id (dsTransID) required by Stripe; use external_id as placeholder
        // until the real PAAY dsTransID is threaded through the charge bundle.
        pi_params.push((
            "payment_method_options[card][three_d_secure][transaction_id]".to_string(),
            external_id.to_string(),
        ));
    }

    tracing::info!(
        "[STRIPE] POST /v1/payment_intents — amount={} currency={} payment_method={} \
         confirm=true three_d_secure={} idempotency-key=zpi-pi-{}",
        amount_cents,
        numeric_to_alpha_currency(currency_numeric),
        pi_params.iter().find(|(k, _)| k == "payment_method").map(|(_, v)| v.as_str()).unwrap_or("?"),
        cavv.map(|_| "yes").unwrap_or("no"),
        external_id,
    );

    let response = client
        .post("https://api.stripe.com/v1/payment_intents")
        .basic_auth(secret_key, Some(""))
        .header("Stripe-Version", "2024-06-20")
        .header("Idempotency-Key", format!("zpi-pi-{}", external_id))
        .form(&pi_params)
        .send()
        .await
        .map_err(|e| format!("Stripe request failed: {}", e))?;

    let http_status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);

    tracing::info!(
        "[STRIPE] POST /v1/payment_intents — HTTP {} body={}",
        http_status,
        body,
    );

    if !http_status.is_success() {
        let err_msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Stripe error");
        if mock_success {
            let mock_id = format!("pi_mock_{}", external_id);
            tracing::warn!(
                "[STRIPE] mock-success: swallowing PI error (HTTP {}: {}) → id={}",
                http_status, err_msg, mock_id
            );
            return Ok(mock_id);
        }
        return Err(format!(
            "Stripe charge failed (HTTP {}): {}",
            http_status, err_msg
        ));
    }

    let pi_id = body["id"].as_str().unwrap_or("").to_string();
    let pi_status = body["status"].as_str().unwrap_or("unknown");

    tracing::info!(
        "[STRIPE] PaymentIntent created: id={}, status={}",
        pi_id,
        pi_status,
    );

    match pi_status {
        "succeeded" | "requires_capture" => Ok(pi_id),
        other => Err(format!(
            "PaymentIntent ended in unexpected status '{}' (id={})",
            other, pi_id
        )),
    }
}

/// Convert ISO 4217 numeric currency code to alphabetic (lowercase).
pub fn numeric_to_alpha_currency(numeric: &str) -> String {
    match numeric {
        "840" => "usd".to_string(),
        "978" => "eur".to_string(),
        "826" => "gbp".to_string(),
        "392" => "jpy".to_string(),
        "036" => "aud".to_string(),
        "124" => "cad".to_string(),
        "756" => "chf".to_string(),
        "356" => "inr".to_string(),
        "156" => "cny".to_string(),
        "702" => "sgd".to_string(),
        _ => numeric.to_lowercase(),
    }
}

/// Constant-time byte slice comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Minimal HTML escaping for strings inserted into HTML templates.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
