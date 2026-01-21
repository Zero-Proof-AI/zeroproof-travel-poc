/// Agent A MCP Client library
/// Exposes core orchestration logic for reuse in CLI and HTTP server modes

pub mod orchestration;
pub mod proxy_fetch;
pub mod shared;
pub mod prompts;
pub mod booking;
pub mod payment;

pub use orchestration::{AgentConfig, BookingState, ClaudeMessage, process_user_query};
pub use shared::{submit_proof_to_database, CryptographicProof, RedactionMetadata};
