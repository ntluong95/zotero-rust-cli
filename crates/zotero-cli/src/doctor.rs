//! Port of `core/doctor.py` runtime health checks for agent-facing use.

use serde_json::{json, Value};

use crate::bridge::client::JSBridgeClient;
use crate::paths;
use crate::runtime::RuntimeContext;

/// `run_doctor()` (`doctor.py:13-133`): aggregate connector / local API / plugin / bridge health.
pub fn run_doctor(runtime: &RuntimeContext, bridge: &JSBridgeClient) -> Value {
    let profile_dir = runtime.environment.profile_dir.as_deref();
    let xpi_path = paths::plugin_xpi_path(profile_dir);
    let installed = paths::plugin_installed(profile_dir);
    let installed_version = paths::installed_plugin_version(profile_dir);
    let bundled_version = paths::bundled_plugin_version();
    let update_available = installed_version.is_some()
        && bundled_version.is_some()
        && installed_version != bundled_version;
    // One probe, two facts: whether the endpoint is ours, and whether anything answers it at
    // all. Asking twice would double this diagnostic's request count for no new information.
    let probe = bridge.probe_bridge();
    let active = probe == crate::bridge::BridgeProbe::Owned;

    let mut js_ok = false;
    let mut js_error: Option<String> = None;
    let mut js_result: Option<Value> = None;
    let mut zotero_js_version: Option<String> = None;

    if active {
        let result = bridge.execute_js_http_required(
            "return {ok: true, value: 'cli-bridge-ok', version: Zotero.version};",
            5,
        );
        let data = result.data.clone();
        js_ok = result.ok
            && data
                .as_ref()
                .and_then(|d| d.get("value"))
                .and_then(Value::as_str)
                == Some("cli-bridge-ok");
        js_result = data.clone();
        js_error = result.error;
        if let Some(ref d) = data {
            if let Some(v) = d.get("version").and_then(Value::as_str) {
                zotero_js_version = Some(v.to_string());
            }
        }
    }

    let zotero_version_opt =
        if runtime.environment.version.is_empty() || runtime.environment.version == "unknown" {
            None
        } else {
            Some(runtime.environment.version.clone())
        };

    // The five plugin/Bridge states the CLI must be able to tell apart, so a diagnostic never
    // collapses "not installed" and "installed but Zotero is closed" into one unhelpful boolean:
    //
    //   not_installed              -- no XPI in the profile's extensions directory
    //   installed_zotero_closed    -- XPI present, but nothing answers Zotero's HTTP port
    //   installed_not_loaded       -- Zotero is up, but /cli-bridge/eval does not answer
    //   ownership_invalid          -- the endpoint answered but failed the fork+id handshake
    //   healthy                    -- owned endpoint answered and an eval round-tripped
    let bridge_state = if !installed {
        "not_installed"
    } else if !runtime.zotero_http_responding() {
        "installed_zotero_closed"
    } else if !active {
        // Not ours: either the fork/id handshake failed (something else serves that path) or
        // nothing answered at all. The single probe above already distinguishes the two.
        if probe == crate::bridge::BridgeProbe::Foreign {
            "ownership_invalid"
        } else {
            "installed_not_loaded"
        }
    } else if js_ok {
        "healthy"
    } else {
        "eval_failing"
    };

    let checks = json!({
        "package": {
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "zotero_app": {
            "ok": zotero_version_opt.is_some(),
            "version": zotero_version_opt,
            "executable": runtime.environment.executable.as_ref().map(|p| p.to_string_lossy()),
            "data_dir": Some(runtime.environment.data_dir.to_string_lossy()),
            "profile_dir": profile_dir.map(|p| p.to_string_lossy()),
        },
        "connector": {
            "ok": runtime.connector_available,
            "message": runtime.connector_message,
        },
        "local_api": {
            "ok": runtime.local_api_available,
            "message": runtime.local_api_message,
            "configured": runtime.environment.local_api_enabled_configured,
        },
        "plugin": {
            "ok": installed && !update_available,
            "xpi_installed": installed,
            "xpi_path": xpi_path.as_ref().map(|p| p.to_string_lossy()),
            "installed_version": installed_version,
            "bundled_version": bundled_version,
            "update_available": update_available,
        },
        "bridge": {
            "ok": active && js_ok,
            "endpoint_active": active,
            "js_ok": js_ok,
            "js_error": js_error,
            "js_result": js_result,
            "zotero_js_version": zotero_js_version,
            // Additive field: which of the five plugin/Bridge states this actually is. The
            // pre-existing booleans keep their exact meanings, so no consumer of the published
            // schema breaks.
            "state": bridge_state,
            // The port every Bridge-routed command in this process will use. Surfacing it makes
            // a port mismatch visible in the diagnostic itself rather than only as a downstream
            // "endpoint not available" from an unrelated command.
            "port": runtime.environment.port,
        },
    });

    let mut next_steps: Vec<String> = Vec::new();
    if !runtime.connector_available {
        next_steps.push("Start Zotero desktop (connector is not available).".to_string());
    }
    if !runtime.local_api_available {
        next_steps.push(
            "Enable Local API: zotero-cli app enable-local-api --launch (or Zotero Settings → Advanced → allow other apps)."
                .to_string(),
        );
    }
    if !installed {
        next_steps.push(
            "Install CLI Bridge: zotero-cli app install-plugin, then restart Zotero.".to_string(),
        );
    } else if update_available {
        next_steps.push(format!(
            "Upgrade CLI Bridge {} → {}: zotero-cli app install-plugin, then restart Zotero.",
            installed_version.as_deref().unwrap_or(""),
            bundled_version.as_deref().unwrap_or("")
        ));
    } else if !active {
        // One remediation per distinguishable state -- "restart Zotero" is the wrong advice for
        // a closed Zotero and useless advice for a foreign endpoint serving that path.
        next_steps.push(
            match bridge_state {
                "installed_zotero_closed" => {
                    "CLI Bridge is installed but Zotero is not running. Start Zotero, or run: \
                     zotero-cli app launch."
                }
                "ownership_invalid" => {
                    "Something is serving /cli-bridge/eval but it is not this CLI's Bridge \
                     plugin. Reinstall it: zotero-cli app install-plugin, then restart Zotero."
                }
                _ => "Restart Zotero so /cli-bridge/eval is registered.",
            }
            .to_string(),
        );
    } else if !js_ok {
        next_steps.push(
            "Bridge endpoint is up but eval failed; reinstall plugin and restart Zotero."
                .to_string(),
        );
    }

    let ready = checks["package"]["ok"] == true
        && checks["zotero_app"]["ok"] == true
        && checks["connector"]["ok"] == true
        && checks["local_api"]["ok"] == true
        && checks["plugin"]["ok"] == true
        && checks["bridge"]["ok"] == true;

    // `write_ready` must mean exactly "a write this CLI can actually perform will not fail for
    // lack of a backend", so it is the disjunction of the two approved write backends -- not a
    // coarse conjunction of unrelated surfaces.
    //
    // The RC1 definition (`connector && plugin && bridge`) produced false-ready states in both
    // directions: it reported `true` on an install whose Local API was unreachable and whose
    // Bridge answered on a different port than the write commands used, and it would report
    // `false` on a perfectly usable Local-API-only install that has no plugin at all. It also
    // said nothing about *authorization*, which is a separate fact and is reported separately.
    let bridge_write_ready = checks["bridge"]["ok"] == true;
    let local_api_write_ready = runtime.local_api_writes_available;
    let write_ready = bridge_write_ready || local_api_write_ready;

    let read_ready = checks["zotero_app"]["ok"] == true
        && !runtime.environment.sqlite_path.as_os_str().is_empty();

    let next_steps_val = if next_steps.is_empty() {
        vec!["CLI Bridge and local surfaces look healthy.".to_string()]
    } else {
        next_steps
    };

    let summary = if ready {
        "All systems ready for agent read/write."
    } else {
        "Some surfaces are unavailable; see checks and next_steps."
    };

    json!({
        "action": "app_doctor",
        "ok": ready,
        "status": if ready { "ready" } else { "degraded" },
        "code": if ready { "READY" } else { "DEGRADED" },
        "ready": ready,
        "read_ready": read_ready,
        "write_ready": write_ready,
        // Which backend `write_ready` is actually claiming, so a caller never has to guess
        // whether an authorized Local API, the Bridge, or both are behind a bare `true`.
        // `local_api_write_authorized` is deliberately about capability, never about the
        // credential itself: no key material reaches this payload.
        "write_backends": {
            "bridge": bridge_write_ready,
            "local_api": local_api_write_ready,
        },
        "checks": checks,
        "next_steps": next_steps_val,
        "summary": summary,
    })
}

/// `plugin_version_warning()` (`doctor.py:136-158`).
pub fn plugin_version_warning(runtime: &RuntimeContext) -> Option<Value> {
    let profile_dir = runtime.environment.profile_dir.as_deref();
    let installed = paths::installed_plugin_version(profile_dir);
    let bundled = paths::bundled_plugin_version();
    if let (Some(inst), Some(bund)) = (&installed, &bundled) {
        if inst != bund {
            return Some(json!({
                "warning": "plugin_version_mismatch",
                "installed_version": inst,
                "bundled_version": bund,
                "message": format!("CLI Bridge plugin {inst} != bundled {bund}. Run: zotero-cli app install-plugin, then restart Zotero."),
            }));
        }
    }
    if !paths::plugin_installed(profile_dir) {
        return Some(json!({
            "warning": "plugin_missing",
            "installed_version": Value::Null,
            "bundled_version": bundled,
            "message": "CLI Bridge plugin not installed. Run: zotero-cli app install-plugin",
        }));
    }
    None
}
