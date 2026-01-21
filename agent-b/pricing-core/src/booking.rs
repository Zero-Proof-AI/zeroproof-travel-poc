use alloc::string::String;
use serde::{Deserialize, Serialize};

#[cfg(feature = "http")]
use serde_json::Value;

#[cfg(feature = "http")]
use alloc::string::ToString;

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub from: String,
    pub to: String,
    pub passenger_name: String,
    pub passenger_email: String,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub booking_id: String,
    pub status: String,
    pub confirmation_code: String,
}

/// Booking logic that runs both on server and inside SP1
/// NOTE: The async version (handle_async) calls https://httpbin.org/json
/// The sync version (handle) returns a deterministic result for SP1
pub fn handle(req: Request) -> Response {
    // Deterministic booking logic for SP1 (no external HTTP calls possible)
    let booking_data = alloc::format!(
        "{}-{}-{}-{}",
        req.from, req.to, req.passenger_name, req.passenger_email
    );
    
    let booking_id = alloc::format!("BK{:08X}", booking_data.len() * 12345);
    let confirmation_code = alloc::format!("CONF{:06X}", booking_data.len() * 67890);

    Response {
        booking_id,
        status: String::from("confirmed"),
        confirmation_code,
    }
}

/// Async version for server: calls https://httpbin.org/json and returns JSON as confirmation code
#[cfg(feature = "http")]
pub async fn handle_async(req: Request) -> Response {
    let mut status = String::from("confirmed");
    // Try to get JSON from httpbin.org and use it for confirmation code
    let confirmation_code = match reqwest::Client::new()
        .get("https://httpbin.org/json")
        .send()
        .await
    {
        Ok(response) => match response.json::<Value>().await {
            Ok(json) => {
                json.to_string()
            },
            Err(_) => {
                status = String::from("failed");
                "ERROR".to_string()
            }
        },
        Err(_) => {
            status = String::from("failed");
            "FAILED TO SEND REQUEST TO AIRLINE".to_string()
        } 
    };

    // Generate deterministic booking ID from request data
    let booking_data = alloc::format!(
        "{}-{}-{}-{}",
        req.from, req.to, req.passenger_name, req.passenger_email
    );
    
    let booking_id = alloc::format!("BK{:08X}", booking_data.len() * 12345);

    Response {
        booking_id,
        status,
        confirmation_code,
    }
}
