/// Agent A MCP Client library
/// Exposes core orchestration logic for reuse in CLI and HTTP server modes

pub mod orchestration;
pub mod proxy_fetch;

pub use orchestration::{AgentConfig, BookingState, ClaudeMessage, process_user_query, CryptographicProof, submit_proof_to_database};
