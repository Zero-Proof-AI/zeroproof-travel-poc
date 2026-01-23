/// Tools management module
/// Handles fetching and caching of tool definitions from all servers

pub mod fetch;
pub mod claude;
pub mod proof;
pub mod proxy_fetch;
pub mod tool_map;

pub use fetch::{fetch_all_tools, fetch_tool_definitions, parse_tool_calls};
pub use claude::{call_claude, call_server_tool, call_tool_with_proof, call_server_tool_with_proof};
pub use proof::{submit_proof, CryptographicProof, RedactionMetadata};
pub use proxy_fetch::{ProxyFetch, ProxyConfig, ZkfetchToolOptions, AttestationConfig};
pub use tool_map::build_tool_options_map;
