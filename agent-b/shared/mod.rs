//! Shared utilities and modules for agent-a and agent-b

pub mod proxy_fetch;
pub mod proof;

pub use proxy_fetch::{ProxyFetch, ProxyConfig, ZkfetchToolOptions, apply_redactions, redact_at_path};
pub use proof::{submit_proof_to_attestation_service, submit_proof, CryptographicProof, RedactionMetadata};
