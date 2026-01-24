//! Shared utilities and modules for agent-a and agent-b

pub mod proxy_fetch;
pub mod proof;
pub mod signature;

pub use proxy_fetch::{ProxyFetch, ProxyConfig, ZkfetchToolOptions, apply_redactions, redact_at_path};
pub use proof::{submit_proof, CryptographicProof, RedactionMetadata};
pub use signature::{verify_secp256k1_sig, verify_proof_gas_free};
