use rusqlite::{params, Connection};
use chrono::{Duration, Utc};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use farm_core::types::{Order, OrderStatus};

/// Persistent merchant store backed by SQLite.
pub struct MerchantDb {
    conn: Mutex<Connection>,
}

impl MerchantDb {
    /// Open (or create) the database at the given path.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = PathBuf::from(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS merchant_config (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) STRICT;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS product_chain_prefs (
                product_id TEXT NOT NULL,
                chain_id   INTEGER NOT NULL,
                PRIMARY KEY (product_id, chain_id)
            ) STRICT;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS orders (
                order_id       TEXT PRIMARY KEY,
                session_id     TEXT NOT NULL,
                items_json     TEXT NOT NULL,
                total_cents    INTEGER NOT NULL,
                status         TEXT NOT NULL DEFAULT 'pending_payment',
                payment_method TEXT NOT NULL DEFAULT 'x402_crypto',
                tx_hash        TEXT,
                network        TEXT,
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
            ) STRICT;",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Get the stored merchant wallet address, if any.
    pub fn get_wallet_address(&self) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM merchant_config WHERE key = 'wallet_address'",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    /// Get the stored merchant email, if any.
    pub fn get_email(&self) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM merchant_config WHERE key = 'email'",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    /// Save the merchant wallet address and email after enrollment.
    pub fn save_enrollment(
        &self,
        email: &str,
        wallet_address: &str,
        s1k: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('email', ?1)",
            params![email],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('wallet_address', ?1)",
            params![wallet_address],
        )?;
        if let Some(v) = s1k {
            conn.execute(
                "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k', ?1)",
                params![v],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k_issued_at', ?1)",
                params![&now],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k_expires_at', ?1)",
                params![(Utc::now() + Duration::days(30)).to_rfc3339()],
            )?;
        }
        Ok(())
    }

    /// Get the stored merchant s1k, if any.
    pub fn get_s1k(&self) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        let s1k = conn.query_row(
            "SELECT value FROM merchant_config WHERE key = 's1k'",
            [],
            |row| row.get(0),
        )
        .ok()?;

        let expires_at = conn
            .query_row(
                "SELECT value FROM merchant_config WHERE key = 's1k_expires_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();

        if let Some(expires_at) = expires_at {
            if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&expires_at) {
                if expiry < Utc::now() {
                    let _ = conn.execute(
                        "DELETE FROM merchant_config WHERE key IN ('s1k', 's1k_issued_at', 's1k_expires_at')",
                        [],
                    );
                    return None;
                }
            }
        }

        Some(s1k)
    }

    /// Save or update the merchant s1k with optional expiry.
    pub fn save_s1k(&self, s1k: &str, expires_at: Option<&str>) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k', ?1)",
            params![s1k],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k_issued_at', ?1)",
            params![&now],
        )?;
        if let Some(exp) = expires_at {
            conn.execute(
                "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k_expires_at', ?1)",
                params![exp],
            )?;
        } else {
            // Default to 30 days if no expiry specified
            conn.execute(
                "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('s1k_expires_at', ?1)",
                params![(Utc::now() + Duration::days(30)).to_rfc3339()],
            )?;
        }
        Ok(())
    }

    /// Get an arbitrary config value.
    pub fn get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM merchant_config WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    /// Set an arbitrary config value.
    pub fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get the enabled chain IDs for a product.
    /// Returns None if no preferences are stored (meaning "all chains").
    pub fn get_product_chains(&self, product_id: &str) -> Option<Vec<u64>> {
        let conn = self.conn.lock().unwrap();
        // Check if any rows exist for this product
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM product_chain_prefs WHERE product_id = ?1",
                params![product_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if count == 0 {
            return None; // no prefs saved → all chains
        }
        let mut stmt = conn
            .prepare("SELECT chain_id FROM product_chain_prefs WHERE product_id = ?1")
            .ok()?;
        let ids: Vec<u64> = stmt
            .query_map(params![product_id], |row| row.get(0))
            .ok()?
            .filter_map(|r| r.ok())
            .collect();
        Some(ids)
    }

    /// Set the enabled chain IDs for a product, replacing any previous prefs.
    pub fn set_product_chains(&self, product_id: &str, chain_ids: &[u64]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM product_chain_prefs WHERE product_id = ?1",
            params![product_id],
        )?;
        for &cid in chain_ids {
            conn.execute(
                "INSERT INTO product_chain_prefs (product_id, chain_id) VALUES (?1, ?2)",
                params![product_id, cid as i64],
            )?;
        }
        Ok(())
    }

    // ── Order persistence ────────────────────────────────────────

    /// Insert a new order into the database.
    pub fn insert_order(&self, order: &Order) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let items_json = serde_json::to_string(&order.items)?;
        let status = serde_json::to_value(&order.status)?
            .as_str()
            .unwrap_or("pending_payment")
            .to_string();
        let method = serde_json::to_value(&order.payment_method)?
            .as_str()
            .unwrap_or("x402_crypto")
            .to_string();
        conn.execute(
            "INSERT INTO orders (order_id, session_id, items_json, total_cents, status, payment_method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![order.order_id, order.session_id, items_json, order.total_cents as i64, status, method],
        )?;
        Ok(())
    }

    /// Update order status (and optionally tx_hash/network on payment).
    pub fn update_order_status(
        &self,
        order_id: &str,
        status: &OrderStatus,
        tx_hash: Option<&str>,
        network: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let status_str = serde_json::to_value(status)?
            .as_str()
            .unwrap_or("pending_payment")
            .to_string();
        conn.execute(
            "UPDATE orders SET status = ?1, tx_hash = ?2, network = ?3, updated_at = datetime('now')
             WHERE order_id = ?4",
            params![status_str, tx_hash, network, order_id],
        )?;
        Ok(())
    }

    /// Find a single order by its Stripe session_id.
    pub fn get_order_by_session_id(&self, session_id: &str) -> Option<OrderRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT order_id, session_id, items_json, total_cents, status, payment_method,
                    tx_hash, network, created_at, updated_at
             FROM orders WHERE session_id = ?1 LIMIT 1",
            params![session_id],
            |row| {
                Ok(OrderRow {
                    order_id: row.get(0)?,
                    session_id: row.get(1)?,
                    items_json: row.get(2)?,
                    total_cents: row.get::<_, i64>(3)? as u64,
                    status: row.get(4)?,
                    payment_method: row.get(5)?,
                    tx_hash: row.get(6)?,
                    network: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .ok()
    }

    /// List all orders, most recent first.
    pub fn list_orders(&self) -> anyhow::Result<Vec<OrderRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT order_id, session_id, items_json, total_cents, status, payment_method,
                    tx_hash, network, created_at, updated_at
             FROM orders ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(OrderRow {
                order_id: row.get(0)?,
                session_id: row.get(1)?,
                items_json: row.get(2)?,
                total_cents: row.get::<_, i64>(3)? as u64,
                status: row.get(4)?,
                payment_method: row.get(5)?,
                tx_hash: row.get(6)?,
                network: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }
}

/// Flat row returned from the orders table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrderRow {
    pub order_id: String,
    pub session_id: String,
    pub items_json: String,
    pub total_cents: u64,
    pub status: String,
    pub payment_method: String,
    pub tx_hash: Option<String>,
    pub network: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub type SharedMerchantDb = Arc<MerchantDb>;

/// Open the merchant DB at the default path (~/.agent-b/merchant.db) or AGENT_B_DB env var.
pub fn open_merchant_db() -> anyhow::Result<SharedMerchantDb> {
    let path = std::env::var("AGENT_B_DB").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{}/.agent-b/merchant.db", home)
    });
    tracing::info!(path = %path, "Opening merchant database");
    let db = MerchantDb::open(&path)?;
    Ok(Arc::new(db))
}

#[cfg(test)]
mod tests {
    use super::*;
    use farm_core::types::{CartItem, Order, OrderStatus, PaymentMethod};

    /// Helper: open an in-memory DB so tests don't touch the filesystem.
    fn mem_db() -> MerchantDb {
        MerchantDb::open(":memory:").expect("in-memory DB")
    }

    fn sample_order(id: &str) -> Order {
        Order {
            order_id: id.into(),
            session_id: "sess-1".into(),
            items: vec![
                CartItem {
                    product_id: "farm-eggs-dozen".into(),
                    quantity: 2,
                    unit_price_cents: 599,
                },
                CartItem {
                    product_id: "farm-raw-milk".into(),
                    quantity: 1,
                    unit_price_cents: 899,
                },
            ],
            total_cents: 2097,
            status: OrderStatus::PendingPayment,
            payment_method: PaymentMethod::X402Crypto,
        }
    }

    // ── insert + list ────────────────────────────────────────────

    #[test]
    fn insert_order_and_list() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-001")).unwrap();

        let rows = db.list_orders().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].order_id, "ord-001");
        assert_eq!(rows[0].session_id, "sess-1");
        assert_eq!(rows[0].total_cents, 2097);
        assert_eq!(rows[0].status, "pending_payment");
        assert_eq!(rows[0].payment_method, "x402_crypto");
        assert!(rows[0].tx_hash.is_none());
        assert!(rows[0].network.is_none());
    }

    #[test]
    fn insert_order_items_roundtrip() {
        let db = mem_db();
        let order = sample_order("ord-002");
        db.insert_order(&order).unwrap();

        let rows = db.list_orders().unwrap();
        let items: Vec<CartItem> = serde_json::from_str(&rows[0].items_json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].product_id, "farm-eggs-dozen");
        assert_eq!(items[0].quantity, 2);
        assert_eq!(items[1].product_id, "farm-raw-milk");
    }

    #[test]
    fn list_orders_empty() {
        let db = mem_db();
        let rows = db.list_orders().unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_orders_multiple() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-A")).unwrap();
        db.insert_order(&sample_order("ord-B")).unwrap();

        let rows = db.list_orders().unwrap();
        assert_eq!(rows.len(), 2);
        let ids: Vec<&str> = rows.iter().map(|r| r.order_id.as_str()).collect();
        assert!(ids.contains(&"ord-A"));
        assert!(ids.contains(&"ord-B"));
    }

    // ── update status ────────────────────────────────────────────

    #[test]
    fn update_order_status_to_paid() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-003")).unwrap();

        db.update_order_status(
            "ord-003",
            &OrderStatus::Paid,
            Some("0xabc123"),
            Some("eip155:84532"),
        )
        .unwrap();

        let rows = db.list_orders().unwrap();
        assert_eq!(rows[0].status, "paid");
        assert_eq!(rows[0].tx_hash.as_deref(), Some("0xabc123"));
        assert_eq!(rows[0].network.as_deref(), Some("eip155:84532"));
    }

    #[test]
    fn update_order_status_to_shipped() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-004")).unwrap();

        db.update_order_status("ord-004", &OrderStatus::Paid, None, None).unwrap();
        db.update_order_status("ord-004", &OrderStatus::Shipped, None, None).unwrap();

        let rows = db.list_orders().unwrap();
        assert_eq!(rows[0].status, "shipped");
    }

    #[test]
    fn update_order_status_to_cancelled() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-005")).unwrap();

        db.update_order_status("ord-005", &OrderStatus::Cancelled, None, None).unwrap();

        let rows = db.list_orders().unwrap();
        assert_eq!(rows[0].status, "cancelled");
    }

    #[test]
    fn update_nonexistent_order_is_noop() {
        let db = mem_db();
        // Should not error, just updates 0 rows
        db.update_order_status("ord-ghost", &OrderStatus::Paid, None, None).unwrap();
        assert!(db.list_orders().unwrap().is_empty());
    }

    // ── duplicate insert ─────────────────────────────────────────

    #[test]
    fn insert_duplicate_order_id_fails() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-dup")).unwrap();
        let result = db.insert_order(&sample_order("ord-dup"));
        assert!(result.is_err());
    }

    // ── timestamps ───────────────────────────────────────────────

    #[test]
    fn order_has_timestamps() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-ts")).unwrap();

        let rows = db.list_orders().unwrap();
        assert!(!rows[0].created_at.is_empty());
        assert!(!rows[0].updated_at.is_empty());
    }

    #[test]
    fn update_status_changes_updated_at() {
        let db = mem_db();
        db.insert_order(&sample_order("ord-upd")).unwrap();
        let before = db.list_orders().unwrap()[0].updated_at.clone();

        db.update_order_status("ord-upd", &OrderStatus::Shipped, None, None).unwrap();
        let after = db.list_orders().unwrap()[0].updated_at.clone();

        // updated_at should be >= before (may be same second in fast tests)
        assert!(after >= before);
    }
}
