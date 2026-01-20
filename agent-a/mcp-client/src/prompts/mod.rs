/// Prompts module
/// Centralized management of all Claude prompts for extraction and processing

pub mod extraction;

pub use extraction::{
    get_passenger_name_extraction_prompt,
    get_passenger_email_extraction_prompt,
    get_payment_method_extraction_prompt,
    extract_with_claude,
    EXTRACTION_SYSTEM_PROMPT,
};
