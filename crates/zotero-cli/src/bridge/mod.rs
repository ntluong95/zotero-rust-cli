#![allow(dead_code, unused_imports)]

pub mod client;
pub mod templates;
pub mod types;

pub use client::{clear_probe_cache, format_bridge_error, JSBridgeClient, DEFAULT_PORT};
pub use types::{BridgeResponse, OwnershipMarker, WriteOutcome};

pub fn default_port() -> u16 {
    JSBridgeClient::with_default_port().port
}

pub fn bridge_endpoint_active() -> bool {
    JSBridgeClient::with_default_port().bridge_endpoint_active()
}

pub fn execute_js(code: &str, wait_seconds: u64) -> BridgeResponse {
    JSBridgeClient::with_default_port().execute_js(code, wait_seconds)
}
