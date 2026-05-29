use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Dairy,
    Meat,
    Poultry,
    Produce,
}

impl Category {
    pub fn as_str(&self) -> &str {
        match self {
            Category::Dairy => "dairy",
            Category::Meat => "meat",
            Category::Poultry => "poultry",
            Category::Produce => "produce",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: Category,
    pub price_cents: u64,
    pub unit: String,
    pub in_stock: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CartItem {
    pub product_id: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cart {
    pub session_id: String,
    pub items: Vec<CartItem>,
    pub total_cents: u64,
}

impl Cart {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            items: Vec::new(),
            total_cents: 0,
        }
    }

    pub fn recalculate_total(&mut self) {
        self.total_cents = self
            .items
            .iter()
            .map(|item| item.unit_price_cents * item.quantity as u64)
            .sum();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    PendingPayment,
    Paid,
    Shipped,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    X402Crypto,
    CreditCard,
    Plaid,
    Stripe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    pub order_id: String,
    pub session_id: String,
    pub items: Vec<CartItem>,
    pub total_cents: u64,
    pub status: OrderStatus,
    pub payment_method: PaymentMethod,
}
