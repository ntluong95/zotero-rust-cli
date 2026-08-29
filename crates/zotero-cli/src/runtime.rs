//! Port of `core/discovery.py`'s `RuntimeContext` (only `build_runtime_context`
//! and `to_status_payload` — `launch_zotero`/`ensure_*_ready` are not part
//! of the read-only vertical slice).

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::http;
use crate::paths::{self, ZoteroEnvironment};

pub struct RuntimeContext {
    pub environment: ZoteroEnvironment,
    pub backend: String,
    pub connector_available: bool,
    pub connector_message: String,
    pub local_api_available: bool,
    pub local_api_message: String,
    /// The running Zotero's `Zotero-Server-ID` header, when the Local API
    /// port answered at all (present even when the Local API itself is
    /// disabled and returns 403 -- see `http::probe_local_api`). `None`
    /// only when the port didn't answer or predates Zotero 10, which don't
    /// send this header. No Python equivalent: additive-only field, see
    /// `phase-14-zotero-10-compatibility-gate.md` §4.
    pub server_id: Option<String>,
    /// Whether Local API writes can be routed right now: Zotero 10+
    /// (`server_id` present) *and* the Local API is actually reachable
    /// (`local_api_available`) -- a disabled Local API can't accept writes
    /// even on a 10+ install. Drives Phase 6's Local-API-first-vs-JS-Bridge
    /// write routing decision; not itself wired to any write path here.
    pub local_api_writes_available: bool,
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    #[serde(flatten)]
    environment: ZoteroEnvironment,
    backend: String,
    connector_available: bool,
    connector_message: String,
    local_api_available: bool,
    local_api_message: String,
    server_id: Option<String>,
    local_api_writes_available: bool,
}

impl RuntimeContext {
    /// `to_status_payload()` (`discovery.py:22-33`): environment fields
    /// first, then the four probe fields, in that exact order — matches
    /// `app__status.json`'s golden field order (`#[serde(flatten)]`
    /// preserves the struct's own declared field order).
    pub fn to_status_payload(&self) -> Map<String, Value> {
        let payload = StatusPayload {
            environment: self.environment.clone(),
            backend: self.backend.clone(),
            connector_available: self.connector_available,
            connector_message: self.connector_message.clone(),
            local_api_available: self.local_api_available,
            local_api_message: self.local_api_message.clone(),
            server_id: self.server_id.clone(),
            local_api_writes_available: self.local_api_writes_available,
        };
        match serde_json::to_value(payload) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }
}

pub struct BuildEnvironmentArgs<'a> {
    pub backend: &'a str,
    pub data_dir: Option<&'a str>,
    pub profile_dir: Option<&'a str>,
    pub executable: Option<&'a str>,
}

/// `build_runtime_context()` (`discovery.py:36-51`): builds the environment
/// then unconditionally probes both connector and Local API, matching the
/// exact 2-call `http_calls` sequence seen in every golden fixture
/// regardless of whether the invoked command needs HTTP at all.
pub fn build_runtime_context(args: BuildEnvironmentArgs) -> RuntimeContext {
    let env_vars: HashMap<String, String> = paths::current_env_map();
    let environment =
        paths::build_environment(args.data_dir, args.profile_dir, args.executable, &env_vars);
    let (connector_available, connector_message) =
        http::connector_is_available(environment.port, Duration::from_secs(3));
    let local_api_probe = http::probe_local_api(environment.port, Duration::from_secs(3));
    let local_api_writes_available =
        local_api_probe.server_id.is_some() && local_api_probe.available;
    RuntimeContext {
        environment,
        backend: args.backend.to_string(),
        connector_available,
        connector_message,
        local_api_available: local_api_probe.available,
        local_api_message: local_api_probe.message,
        server_id: local_api_probe.server_id,
        local_api_writes_available,
    }
}
