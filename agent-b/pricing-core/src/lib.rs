#![cfg_attr(not(feature = "http"), no_std)]   // No-std for SP1; std when "http" feature enabled

extern crate alloc;
use alloc::string::String;
use serde::{Deserialize, Serialize};

pub mod pricing;
pub mod booking;

/// Single enum — one input type for the entire backend
#[derive(Serialize, Deserialize)]
pub enum RpcCall {
    GetPrice(pricing::Request),
    BookFlight(booking::Request),
}

/// Single enum — one output type
#[derive(Serialize, Deserialize)]
pub enum RpcResult {
    Price(pricing::Response),
    Booking(booking::Response),
    Error(String),
}

/// Main dispatcher — runs both on server and inside SP1
pub fn handle_call(call: RpcCall) -> RpcResult {
    match call {
        RpcCall::GetPrice(req)   => RpcResult::Price(pricing::handle(req)),
        RpcCall::BookFlight(req) => RpcResult::Booking(booking::handle(req)),
    }
}