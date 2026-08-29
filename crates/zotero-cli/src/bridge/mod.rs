#![allow(dead_code, unused_imports)]

pub mod client;
pub mod templates;
pub mod types;

pub use client::{clear_probe_cache, format_bridge_error, JSBridgeClient, DEFAULT_PORT};
pub use types::{BridgeResponse, OwnershipMarker};
// The single canonical write-outcome contract (`phase-06` §3.13), shared with the Local API
// write path (`write.rs`) -- Bridge no longer maintains its own duplicate type. Re-exported here
// so existing `bridge::WriteOutcome` call sites keep working unchanged.
//
// Uses `crate::write`, not an external `zotero_cli::write` reference: this module is compiled
// both as a real child module of this crate (once Slice 6 registers `pub mod bridge;` in
// lib.rs) and, today, via `#[path]` inclusion inside each integration-test binary. `crate::`
// resolves correctly in both contexts as long as the including crate provides a `write` module
// at its own root -- see each `#[path = "../src/bridge/mod.rs"] mod bridge;` test file's own
// `pub use zotero_cli::write;` shim for the test-binary case.
pub use crate::write::WriteOutcome;

pub fn default_port() -> u16 {
    JSBridgeClient::with_default_port().port
}

pub fn bridge_endpoint_active() -> bool {
    JSBridgeClient::with_default_port().bridge_endpoint_active()
}

pub fn execute_js(code: &str, wait_seconds: u64) -> BridgeResponse {
    JSBridgeClient::with_default_port().execute_js(code, wait_seconds)
}
