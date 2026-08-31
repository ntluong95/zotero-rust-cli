//! Port of `core/doctor.py` runtime health checks for agent-facing use.

use serde_json::{json, Value};

use crate::bridge::client::JSBridgeClient;
use crate::paths;
use crate::runtime::RuntimeContext;

/// `run_doctor()` (`doctor.py:13-133`): aggregate connector / local API / plugin / bridge health.
pub fn run_doctor(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    staging_dir: &std::path::Path,
) -> Value {
    let profile_dir = runtime.environment.profile_dir.as_deref();
    let xpi_path = paths::plugin_xpi_path(profile_dir);
    let staged_xpi = crate::plugin::staged_xpi_path(staging_dir);
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
    //   not_installed              -- no XPI in the profile's extensions directory, none staged
    //   staged_not_installed       -- the bundled XPI has been staged, but Zotero's own install
    //                                 dialog has not been completed yet
    //   installed_zotero_closed    -- XPI present, but nothing answers Zotero's HTTP port
    //   installed_not_loaded       -- Zotero is up, but /cli-bridge/eval does not answer
    //   ownership_invalid          -- the endpoint answered but failed the fork+id handshake
    //   healthy                    -- owned endpoint answered and an eval round-tripped
    let bridge_state = if !installed {
        if staged_xpi.is_some() {
            "staged_not_installed"
        } else {
            "not_installed"
        }
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
            // Where the bundled XPI is waiting, when it has been staged but not yet installed --
            // so the next step can name the exact file to select in Zotero's dialog.
            "staged_xpi_path": staged_xpi.as_ref().map(|p| p.to_string_lossy()),
        },
    });

    // Next steps are generated from the *combination* of facts, not from each check in
    // isolation. The previous version keyed the Local API advice off `local_api_available`
    // (reachability) alone, so a perfectly configured install with Zotero merely closed was told
    // to "enable" what it had already enabled -- and told to do it with `app enable-local-api`,
    // a command this CLI deliberately does not implement (the canonical behavior is Excluded on
    // safety grounds; `app authorize-local-api` is the approved consent path). Following that
    // advice was impossible, and pointed at the very workflow the exclusion exists to prevent.
    let zotero_running = runtime.zotero_http_responding();
    let mut next_steps: Vec<String> = Vec::new();

    if !zotero_running {
        next_steps.push(
            "Zotero is not running. Commands that need a live backend start it automatically; \
             to start it yourself run: zotero-cli app launch."
                .to_string(),
        );
    } else if !runtime.connector_available {
        next_steps.push(format!(
            "Zotero is running but its connector is not answering ({}). Restart Zotero, then \
             re-run: zotero-cli app doctor.",
            runtime.connector_message
        ));
    }

    match (
        runtime.environment.local_api_enabled_configured,
        runtime.local_api_available,
        zotero_running,
    ) {
        // Reachable: nothing to configure. Authorization is a separate fact, reported below.
        (_, true, _) => {}
        // Configured, unreachable, Zotero closed -- the cause is the closed Zotero, already
        // reported above. Saying "enable the Local API" here would be actively wrong.
        (true, false, false) => next_steps.push(
            "The Local API is already enabled in Zotero's settings; it is unavailable only \
             because Zotero is not running."
                .to_string(),
        ),
        // Configured, unreachable, Zotero running -- genuinely unexpected, so say so rather
        // than repeating setup advice the user has already followed.
        (true, false, true) => next_steps.push(
            "The Local API is enabled in Zotero's settings but the running Zotero is not \
             serving it. Restart Zotero, then check Settings → Advanced → \
             \"Allow other applications on this computer to communicate with Zotero\"."
                .to_string(),
        ),
        // Not configured: this is the only case where enabling is the right advice. There is
        // deliberately no CLI command for it -- the setting is changed in Zotero itself.
        (false, false, _) => next_steps.push(
            "Enable the Local API in Zotero: Settings → Advanced → \"Allow other applications \
             on this computer to communicate with Zotero\"."
                .to_string(),
        ),
    }

    if !installed {
        next_steps.push(match staged_xpi.as_ref() {
            // Staged but not installed: name the exact file, because the Zotero dialog asks for
            // a path and hunting for it is the step people get stuck on.
            Some(path) => format!(
                "CLI Bridge is staged but not installed yet. In Zotero: Tools → Plugins → gear \
                 icon → Install Add-on From File… → select {} → restart Zotero.",
                path.display()
            ),
            None => "CLI Bridge is not installed. Some live operations (raw JS, sync, \
                     cross-library search while Zotero is running) require it. Run: \
                     zotero-cli app install-plugin"
                .to_string(),
        });
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
