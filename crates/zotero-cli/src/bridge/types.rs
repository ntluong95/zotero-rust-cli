use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Envelope matching Python JSBridge response `{ok: bool, data: Any, error: str, ...}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_raw: Option<String>,
}

impl BridgeResponse {
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            error_name: None,
            error_stack: None,
            error_raw: None,
        }
    }

    pub fn failure(error: String) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error),
            error_name: None,
            error_stack: None,
            error_raw: None,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.ok
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn require_data(&self) -> Result<&Value> {
        if !self.ok {
            let msg = self.error.as_deref().unwrap_or("Bridge call failed");
            bail!("{msg}");
        }
        match &self.data {
            Some(data) => {
                // If data is an object with nested ok: false, extract error
                if let Some(false) = data.get("ok").and_then(|v| v.as_bool()) {
                    if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
                        bail!("{err}");
                    }
                }
                Ok(data)
            }
            None => bail!("Bridge call returned no data"),
        }
    }
}

/// Ownership marker returned by our forked XPI plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipMarker {
    #[serde(default)]
    pub fork: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub ownership: String,
}
