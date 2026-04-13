use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
};
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

    if req.payment_method != "x402_crypto" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(FarmToolResponse::err(
                400,
                format!("Unsupported payment method '{}'. Currently only 'x402_crypto' is supported.", req.payment_method),
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
    let order = Order {
        order_id: order_id.clone(),
        session_id: req.session_id.clone(),
        items: cart.items.clone(),
        total_cents,
        status: OrderStatus::PendingPayment,
        payment_method: PaymentMethod::X402Crypto,
    };

    // Build x402 PaymentRequired — filter chains by per-product preferences
    let mut config = X402Config::from_env();

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
            "description": "Checkout the cart. Returns HTTP 402 with x402 payment challenge for crypto payment. The AI agent's payment processor (zpi-zkpay) handles the x402 flow.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ID for the cart to checkout"
                    },
                    "payment_method": {
                        "type": "string",
                        "enum": ["x402_crypto"],
                        "description": "Payment method. Currently only x402_crypto is supported.",
                        "default": "x402_crypto"
                    }
                },
                "required": ["session_id"]
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
4. To purchase, call farm-checkout with session_id.
5. farm-checkout returns status 402 with payment_required — pass this to
   zpi-zkpay x402-pay to complete crypto payment.
6. After x402-pay returns PAID, the order is automatically confirmed.
7. Available categories: dairy, meat, poultry, produce.
8. All prices are in USD. Crypto payments settle in USDC on Base Sepolia.
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
