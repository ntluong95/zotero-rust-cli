//! Backend-neutral write-outcome contract (`phase-06-js-bridge-and-injection-hardening.md`
//! §3.13, refined per the Phase 6 Slice 0 live write-consent spike --
//! `plans/research/zotero-10-impact-on-rust-port.md` §8). Every write path (Local API today; JS
//! Bridge and, later, Connector API map their own failure shapes onto the same enum) returns
//! this. No Local-API-specific or bridge-local fields belong here -- that lives in
//! `write_router.rs`/`http.rs`.
//!
//! Deliberately holds no raw credential material: `WriteOutcome`/`CredentialSource` describe
//! *where* a credential came from and *why* a write didn't apply, never the credential itself.

use serde::Serialize;

/// Where a credential considered during a write attempt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// `ZOTERO_LOCAL_API_KEY` -- operator-owned. The CLI never mutates or deletes this source;
    /// on rejection it only reports the failure (`credentials::resolve_credential`/`invalidate`).
    Environment,
    /// The CLI-owned local credential file, scoped to a specific `Zotero-Server-ID`.
    Store,
    /// No credential was available from either source.
    None,
}

/// Why a write did not apply due to Local API write-authorization state. LIVE VERIFIED against a
/// real Zotero 10.0.1 instance unless noted (`zotero-10-impact-on-rust-port.md` §8.1-§8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationReason {
    /// No usable credential exists for the current `Zotero-Server-ID`. Detected as a local
    /// preflight check before any network write is attempted in the common case (§3.4a); the
    /// server's own defensive shape is `401` with body `API key required -- POST
    /// /api/local/authorize to obtain one`, kept here only as a fallback mapping if a write is
    /// somehow attempted with no credential.
    Required,
    /// A previously-usable credential was rejected: `401` with body `Invalid or expired API
    /// key`. Distinct caller action from `Required`: a `Store`-sourced credential must be
    /// deleted (never an `Environment`-sourced one -- see `CredentialSource`).
    Revoked,
    /// `403` -- a human explicitly clicked "Deny" on the consent dialog. DOC-VERIFIED shape only
    /// (official Zotero Local API docs); Slice 0 never deliberately triggered a denial live.
    Denied,
    /// `429` -- Zotero's documented consent-dialog rate limit (5 dialog-showing requests per
    /// minute). DOC-VERIFIED only. Transient and backoff-safe, unlike the three reasons above --
    /// must never be treated as a "needs human action" signal.
    RateLimited,
}

/// The single shared, backend-neutral outcome of any Local API / JS Bridge / Connector write
/// attempt. Dispatch code (Slice 6, out of this crate's current scope) matches over this, never
/// over a backend-specific response type.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    /// Write applied. `affected_key` feeds the post-write re-read/diff (`write_router`'s
    /// compatibility renderer); the renderer produces output from that re-read, never from this
    /// variant's own data.
    Applied { affected_key: String },
    /// Write-authorization failure. `reason` says why, `source` says whose credential was
    /// involved (and therefore whether the CLI is allowed to mutate its own stored state).
    /// Never triggers an automatic re-authorization or an automatic replay of the original write.
    AuthorizationFailed {
        reason: AuthorizationReason,
        source: CredentialSource,
        detail: String,
    },
    /// `428` -- the request itself omitted a required `Zotero-Server-ID` header. LIVE VERIFIED.
    /// A client-side protocol/invariant bug, not a user-facing authorization state; must never
    /// be surfaced as "needs human action."
    PreconditionFailed { detail: String },
    /// `If-Unmodified-Since-Version` conflict. Caller re-reads and may retry with a fresh
    /// version -- does not imply the write landed.
    Conflict { detail: String },
    /// Transport-level or otherwise unexpected failure. LIVE VERIFIED as a genuinely unresolved
    /// commit state was never safely testable (`zotero-10-impact-on-rust-port.md` §8.4 item G) --
    /// must be treated as an unknown-commit-state and must never trigger an automatic retry of a
    /// non-idempotent write.
    TransportError { detail: String },
}
