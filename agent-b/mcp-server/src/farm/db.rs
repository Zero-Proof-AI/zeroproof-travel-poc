use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    pub fn save_enrollment(&self, email: &str, wallet_address: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('email', ?1)",
            params![email],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO merchant_config (key, value) VALUES ('wallet_address', ?1)",
            params![wallet_address],
        )?;
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
