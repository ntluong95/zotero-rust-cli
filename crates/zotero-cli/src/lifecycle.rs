//! Live-backend lifecycle: the single seam every command that requires a running Zotero goes
//! through before touching a live backend.
//!
//! `app_launch::launch_zotero` already did cross-platform discovery, spawning through an
//! injectable [`ProcessSpawner`], and readiness polling -- but it had exactly one caller, the
//! `app launch` command. Any other command that needed a live backend simply failed with
//! "endpoint not available" and left the agent to hand-orchestrate `app launch` -> wait ->
//! `app doctor` -> retry.
//!
//! `launch_zotero` is also the wrong primitive to call directly from a command: it always waits
//! for the Connector and conditionally for the Local API, and *never* for the Bridge eval
//! endpoint -- so a `js` command that used it would return before its own backend was usable.
//! [`ensure_backend`] waits for the specific capability the caller named.
//!
//! Rules this module enforces, in order:
//!
//! - **Already available -> return immediately.** Never spawn a second Zotero.
//! - **Unavailable while Zotero is answering -> never spawn.** A reachable Connector/Local API
//!   means the process is up and the missing capability is a configuration or plugin problem
//!   that a new process would not fix.
//! - **Unavailable and nothing answers -> launch, then wait for that one backend.**
//! - **Never bypasses human consent.** [`Backend::LocalApiWrite`] waits for the Local API to
//!   come up and checks that a credential *exists*; it never calls `write_router::
//!   authorize_interactive`, never auto-approves, and never reads or prints credential material.
//! - **Diagnostics never call this.** `app doctor`/`status`/`ping` observe state, they do not
//!   change it.

use std::time::Duration;

use crate::app_launch::{self, ProcessSpawner};
use crate::error::DomainError;
use crate::http;
use crate::runtime::{self, BuildEnvironmentArgs, RuntimeContext};

/// Set to `1`/`true` to suppress every automatic launch. The integration harness sets it for
/// every test so no automated run can ever spawn a real Zotero, and an operator can set it to
/// keep an agent from opening a GUI on a headless or shared machine.
pub const NO_AUTOLAUNCH_ENV: &str = "ZOTERO_CLI_NO_AUTOLAUNCH";

/// How long to wait for a freshly launched Zotero to expose the requested backend.
/// Overridable through `ZOTERO_CLI_LAUNCH_TIMEOUT` (seconds) so a slow first start -- or a fast
/// deterministic test -- does not need a code change.
const DEFAULT_LAUNCH_TIMEOUT_SECS: u64 = 60;
const LAUNCH_TIMEOUT_ENV: &str = "ZOTERO_CLI_LAUNCH_TIMEOUT";

/// The capability a command actually needs, rather than one coarse "ready" flag. Waiting for the
/// Connector when the command is going to write through the Bridge is exactly the false-ready
/// state this replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `GET /connector/ping` answers 200.
    Connector,
    /// `GET /api/` answers 200 (reachable *and* enabled in preferences).
    LocalApi,
    /// The Local API is reachable on a Zotero 10+ instance **and** a write credential already
    /// exists for this instance's `Zotero-Server-ID`. Missing authorization is reported, never
    /// obtained automatically.
    LocalApiWrite,
    /// `POST /cli-bridge/eval` answers and passes the fork+id ownership handshake.
    Bridge,
    /// At least one *approved* write backend is usable: an authorized Local API write path, or
    /// the owned Bridge. This is not a coarse "ready" flag -- it is the precise requirement of
    /// the CRUD commands that route to whichever of the two is available (`item update`,
    /// `item tag`, `collection rename`, ...). A command committed to one backend names that
    /// backend instead.
    Write,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Backend::Connector => "Zotero connector",
            Backend::LocalApi => "Zotero Local API",
            Backend::LocalApiWrite => "authorized Zotero Local API write access",
            Backend::Bridge => "CLI Bridge eval endpoint",
            Backend::Write => "a Zotero write backend (authorized Local API, or the CLI Bridge)",
        }
    }

    /// Whether this capability is usable right now, given a freshly probed runtime.
    fn available(self, runtime: &RuntimeContext) -> bool {
        match self {
            Backend::Connector => runtime.connector_available,
            Backend::LocalApi => runtime.local_api_available,
            Backend::LocalApiWrite => runtime.local_api_writes_available,
            Backend::Bridge => runtime.bridge_client().bridge_endpoint_active(),
            Backend::Write => {
                runtime.local_api_writes_available
                    || runtime.bridge_client().bridge_endpoint_active()
            }
        }
    }

    /// Blocks until this specific capability answers, or `timeout` elapses.
    fn wait_until_ready(self, runtime: &RuntimeContext, timeout: Duration) -> bool {
        let port = runtime.environment.port;
        match self {
            Backend::Connector => http::wait_for_endpoint(
                port,
                "/connector/ping",
                timeout,
                Duration::from_millis(500),
                &[],
                &[200],
            ),
            Backend::LocalApi | Backend::LocalApiWrite => http::wait_for_endpoint(
                port,
                "/api/",
                timeout,
                Duration::from_millis(500),
                &[("Zotero-API-Version", http::LOCAL_API_VERSION)],
                &[200],
            ),
            // The Bridge handshake is a POST with an ownership check, which `wait_for_endpoint`
            // (a GET-with-status-set poller) cannot express -- polling `bridge_endpoint_active`
            // keeps the ownership gate in the readiness definition instead of accepting any
            // 200 from that path.
            Backend::Bridge => {
                let client = runtime.bridge_client();
                let deadline = std::time::Instant::now() + timeout;
                while std::time::Instant::now() < deadline {
                    if client.probe_bridge_uncached() == crate::bridge::BridgeProbe::Owned {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                false
            }
            // Either write backend satisfies this, so poll both and stop at the first one --
            // waiting for the Bridge alone would time out on an install that only has the
            // Local API, and vice versa.
            Backend::Write => {
                let client = runtime.bridge_client();
                let deadline = std::time::Instant::now() + timeout;
                while std::time::Instant::now() < deadline {
                    if client.probe_bridge_uncached() == crate::bridge::BridgeProbe::Owned {
                        return true;
                    }
                    let probe = http::probe_local_api(port, Duration::from_secs(3));
                    if probe.available && probe.server_id.is_some() {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                false
            }
        }
    }
}

fn env_flag_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let value = value.trim().to_ascii_lowercase();
            !value.is_empty() && value != "0" && value != "false"
        }
        Err(_) => false,
    }
}

fn launch_timeout() -> Duration {
    let secs = std::env::var(LAUNCH_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_LAUNCH_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Ensures `backend` is usable, launching Zotero exactly once if -- and only if -- Zotero
/// appears to be closed. Returns a runtime re-probed after the launch, so the caller's
/// subsequent capability checks (`local_api_writes_available`, the Bridge port, `server_id`)
/// reflect the process that is actually running now.
///
/// Errors are domain errors (exit 1) and name the specific capability, never a generic "not
/// ready": a launch failure, a readiness timeout, and "Zotero is up but this capability is
/// unavailable" are three different situations for the caller.
pub fn ensure_backend(
    runtime: RuntimeContext,
    backend: Backend,
    spawner: &mut dyn ProcessSpawner,
) -> anyhow::Result<RuntimeContext> {
    if backend.available(&runtime) {
        return Ok(runtime);
    }

    // Zotero is answering on its HTTP port, so the process is up and a second one would not
    // help. Hand the runtime back unchanged and let the command report its own, already-specific
    // failure ("Zotero connector is not available: ...", "JS Bridge endpoint not available.
    // Install the CLI Bridge plugin: ...", the Local API write-authorization outcome). This
    // layer owns *lifecycle*, not error wording -- restating a backend failure here would change
    // the published error of every gated command for no diagnostic gain.
    if runtime.zotero_http_responding() {
        return Ok(runtime);
    }

    if env_flag_enabled(NO_AUTOLAUNCH_ENV) {
        return Err(DomainError::new(format!(
            "{} is unavailable and Zotero does not appear to be running. Automatic launch is \
             disabled ({NO_AUTOLAUNCH_ENV} is set); start Zotero yourself, or run: \
             zotero-cli app launch",
            backend.label()
        ))
        .into());
    }

    // `launch_zotero`'s own readiness poll is skipped (`wait_timeout: 0`) -- this waits for the
    // caller's specific backend below instead of for the Connector/Local API pair it hardcodes.
    app_launch::launch_zotero(&runtime, 0, spawner)?;

    if !backend.wait_until_ready(&runtime, launch_timeout()) {
        return Err(DomainError::new(format!(
            "Zotero was launched but {} did not become ready within {}s. {}",
            backend.label(),
            launch_timeout().as_secs(),
            remediation(backend)
        ))
        .into());
    }

    // Re-probe: the pre-launch runtime's `connector_available`/`local_api_available`/`server_id`
    // all describe a process that was not running yet.
    let refreshed = runtime::build_runtime_context(BuildEnvironmentArgs {
        backend: &runtime.backend,
        data_dir: Some(&runtime.environment.data_dir.to_string_lossy()),
        profile_dir: runtime
            .environment
            .profile_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        executable: runtime
            .environment
            .executable
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
    });

    // `LocalApiWrite` can still be unsatisfied after a successful `/api/` wait: the write
    // capability additionally needs a Zotero 10+ `Zotero-Server-ID`. Authorization itself stays
    // the caller's problem, surfaced through the existing `WriteOutcome::AuthorizationFailed`
    // path rather than fabricated here.
    Ok(refreshed)
}

fn remediation(backend: Backend) -> &'static str {
    match backend {
        Backend::Connector => "Check that Zotero finished starting, then retry.",
        Backend::LocalApi => {
            "Enable it: zotero-cli app enable-local-api --launch (or Zotero Settings -> Advanced \
             -> allow other applications on this computer to communicate with Zotero)."
        }
        Backend::LocalApiWrite => {
            "Authorize write access once, with the Zotero consent dialog: \
             zotero-cli app authorize-local-api."
        }
        Backend::Bridge => {
            "Install the CLI Bridge and restart Zotero: zotero-cli app install-plugin, then \
             verify with: zotero-cli app doctor."
        }
        Backend::Write => {
            "Set up one write backend: zotero-cli app authorize-local-api, or \
             zotero-cli app install-plugin followed by a Zotero restart. Diagnose with: \
             zotero-cli app doctor."
        }
    }
}

/// Bridge-only entry point, for the many commands that need the owned Bridge and nothing else
/// from the runtime (`js`, `sync`, `item search-fulltext`, `collection stats`, ...).
///
/// The happy path costs exactly one request: the port comes from the *environment*, which is
/// resolved from the filesystem (`paths::build_environment`) with no HTTP at all, and the only
/// probe is the Bridge's own ownership handshake -- whose positive result is then cached for the
/// process, so the command's first real Bridge call does not re-probe.
///
/// The connector/Local-API probes only happen on the failure path, where they are genuinely
/// needed to tell "Zotero is closed, launch it" from "Zotero is up but the Bridge is not".
pub fn ensure_bridge(
    environment: &crate::paths::ZoteroEnvironment,
    build_runtime: &dyn Fn() -> RuntimeContext,
    spawner: &mut dyn ProcessSpawner,
) -> anyhow::Result<crate::bridge::JSBridgeClient> {
    let client = crate::bridge::JSBridgeClient::new(environment.port);
    match client.probe_bridge() {
        // Owned, or something is serving that path (so Zotero is up and a launch would be
        // wrong). Either way the command proceeds and, in the `Foreign` case, reports the same
        // "endpoint not available" it always has -- the memoized probe means its own
        // availability check costs no second request.
        crate::bridge::BridgeProbe::Owned | crate::bridge::BridgeProbe::Foreign => Ok(client),
        // Nothing answered: this is the only state in which starting Zotero can help.
        crate::bridge::BridgeProbe::Unreachable => {
            let runtime = ensure_backend(build_runtime(), Backend::Bridge, spawner)?;
            Ok(runtime.bridge_client())
        }
    }
}

/// The spawner every real command uses. Kept as a named helper so no command arm constructs
/// `RealProcessSpawner` inline -- tests substitute a fake at the `ensure_backend` boundary.
pub fn real_spawner() -> app_launch::RealProcessSpawner {
    app_launch::RealProcessSpawner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_treats_unset_empty_zero_and_false_as_disabled() {
        // A deliberately narrow truthiness rule: only an explicitly non-empty, non-zero,
        // non-"false" value disables auto-launch, so an accidentally-exported empty variable
        // never silently changes behavior.
        for (value, expected) in [
            ("", false),
            ("0", false),
            ("false", false),
            ("FALSE", false),
            ("1", true),
            ("true", true),
            ("yes", true),
        ] {
            let name = "ZOTERO_CLI_TEST_FLAG_PROBE";
            // SAFETY: single-threaded test, variable is unique to this test.
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag_enabled(name), expected, "value={value:?}");
            unsafe { std::env::remove_var(name) };
        }
    }

    #[test]
    fn every_backend_has_a_distinct_label_and_remediation() {
        let backends = [
            Backend::Connector,
            Backend::LocalApi,
            Backend::LocalApiWrite,
            Backend::Bridge,
            Backend::Write,
        ];
        let labels: Vec<&str> = backends.iter().map(|b| b.label()).collect();
        let remediations: Vec<&str> = backends.iter().map(|b| remediation(*b)).collect();
        for list in [&labels, &remediations] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                list.len(),
                "capability messages must be distinct"
            );
        }
    }
}
