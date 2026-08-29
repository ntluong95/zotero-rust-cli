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
    let (local_api_available, local_api_message) =
        http::local_api_is_available(environment.port, Duration::from_secs(3));
    RuntimeContext {
        environment,
        backend: args.backend.to_string(),
        connector_available,
        connector_message,
        local_api_available,
        local_api_message,
    }
}
