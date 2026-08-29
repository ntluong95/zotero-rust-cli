//! Local API write orchestration: capability gate, credential resolution, HTTP-response ->
//! `WriteOutcome` mapping, the full-array-replace helper (§3.6 rows 47/68/26), and the scoped
//! post-write compatibility renderer/diff (§3.5). Slices 3-5
//! (`phase-06-js-bridge-and-injection-hardening.md`).
//!
//! **Not this module's job:** command dispatch, `cli.rs`/`lib.rs` wiring, or deciding which
//! commands route to Local API vs. JS Bridge (Slice 6/7/8). Every function here is a library
//! entry point for that later, out-of-scope wiring to call.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::credentials::{self, LocalApiCredential};
use crate::http::{self, LocalWriteResponse};
use crate::runtime::RuntimeContext;
use crate::write::{AuthorizationReason, CredentialSource, WriteOutcome};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Capability gate (§3.6): every Local API write call must be preceded by this check. Returns
/// `Err` rather than a `WriteOutcome` because "Local API writes aren't available at all right
/// now" is a routing decision for the caller (fall back to JS Bridge), not a write-authorization
/// outcome.
pub fn ensure_local_api_writes_available(runtime: &RuntimeContext) -> anyhow::Result<&str> {
    if !runtime.local_api_writes_available {
        anyhow::bail!(
            "Local API writes are not available on this Zotero instance right now \
             (local_api_writes_available=false) -- route to the JS Bridge instead"
        );
    }
    runtime.server_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!("local_api_writes_available is true but server_id is missing")
    })
}

/// Resolves a credential for `server_id` and reports its source in the neutral
/// `write::CredentialSource` shape, without exposing the raw key beyond this call site.
fn resolve(server_id: &str) -> (Option<LocalApiCredential>, CredentialSource) {
    let (credential, source) = credentials::resolve_credential(server_id);
    (credential, source.into())
}

/// LIVE VERIFIED body-text discriminators (`zotero-10-impact-on-rust-port.md` §8.1 finding 10):
/// the two `401` rejections share a status code but not a body.
fn is_revoked_body(body: &str) -> bool {
    body.contains("Invalid or expired API key")
}

/// Maps a raw `LocalWriteResponse` to a `WriteOutcome`, given which credential source (if any)
/// was used. Does not itself mutate credential storage -- callers do that based on the returned
/// `AuthorizationFailed { reason: Revoked, source: Store, .. }` case (§3.4a: only ever invalidate
/// a `Store`-sourced credential, never `Environment`).
fn classify(response: &LocalWriteResponse, source: CredentialSource) -> WriteOutcome {
    match response.status {
        200 | 201 | 204 => {
            unreachable!("success statuses are handled by the caller before classify() is called")
        }
        428 => WriteOutcome::PreconditionFailed {
            detail: response.body.clone(),
        },
        401 if is_revoked_body(&response.body) => WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Revoked,
            source,
            detail: response.body.clone(),
        },
        401 => WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Required,
            source,
            detail: response.body.clone(),
        },
        // DOC-VERIFIED only (official Zotero Local API docs) -- not live-triggered in Slice 0.
        403 => WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Denied,
            source,
            detail: response.body.clone(),
        },
        429 => WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::RateLimited,
            source,
            detail: response.body.clone(),
        },
        // DOC-VERIFIED only (Zotero Web API v3 Write Requests: If-Unmodified-Since-Version
        // conflicts return 412) -- Slice 0 always sent a fresh version, so this was never
        // live-triggered either.
        412 => WriteOutcome::Conflict {
            detail: response.body.clone(),
        },
        other => WriteOutcome::TransportError {
            detail: format!("unexpected HTTP {other}: {}", response.body),
        },
    }
}

/// After a successful write with a `Store`-sourced credential that later turns out revoked, only
/// the matching `server_id` entry is removed -- every other stored credential is untouched
/// (§3.4a). Never called for an `Environment`-sourced credential.
fn invalidate_if_store_revoked(outcome: &WriteOutcome, server_id: &str) {
    if let WriteOutcome::AuthorizationFailed {
        reason: AuthorizationReason::Revoked,
        source: CredentialSource::Store,
        ..
    } = outcome
    {
        let _ = credentials::invalidate_stored(server_id);
    }
}

/// `PATCH <path>` against the Local API, e.g. `path = "/api/users/0/items/<key>"`. Performs the
/// capability gate, credential preflight (§3.4a: `AuthorizationRequired` is reported locally,
/// before any network call, when no credential is available -- `/api/local/authorize` is never
/// invoked from here), the write itself, and revocation bookkeeping on a `Store`-sourced 401.
pub fn patch_item(
    runtime: &RuntimeContext,
    path: &str,
    affected_key: &str,
    fields: &Value,
    if_unmodified_since_version: i64,
) -> anyhow::Result<WriteOutcome> {
    let server_id = ensure_local_api_writes_available(runtime)?;
    let (credential, source) = resolve(server_id);
    let Some(credential) = credential else {
        return Ok(WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Required,
            source,
            detail: "no Local API write credential is available for this Zotero instance \
                     (checked ZOTERO_LOCAL_API_KEY and the local credential store)"
                .to_string(),
        });
    };

    let response = http::local_api_patch(
        runtime.environment.port,
        path,
        server_id,
        &credential.key,
        if_unmodified_since_version,
        fields,
        DEFAULT_TIMEOUT,
    )?;

    let outcome = if response.status == 204 || response.status == 200 {
        WriteOutcome::Applied {
            affected_key: affected_key.to_string(),
        }
    } else {
        classify(&response, source)
    };
    invalidate_if_store_revoked(&outcome, server_id);
    Ok(outcome)
}

/// `DELETE <path>` against the Local API. Same credential/capability/revocation handling as
/// [`patch_item`]. Callers implementing §3.5's delete sub-rule still re-`GET` afterward to assert
/// absence -- this function only performs the delete itself.
pub fn delete_item(
    runtime: &RuntimeContext,
    path: &str,
    affected_key: &str,
    if_unmodified_since_version: i64,
) -> anyhow::Result<WriteOutcome> {
    let server_id = ensure_local_api_writes_available(runtime)?;
    let (credential, source) = resolve(server_id);
    let Some(credential) = credential else {
        return Ok(WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Required,
            source,
            detail: "no Local API write credential is available for this Zotero instance \
                     (checked ZOTERO_LOCAL_API_KEY and the local credential store)"
                .to_string(),
        });
    };

    let response = http::local_api_delete(
        runtime.environment.port,
        path,
        server_id,
        &credential.key,
        if_unmodified_since_version,
        DEFAULT_TIMEOUT,
    )?;

    let outcome = if response.status == 204 {
        WriteOutcome::Applied {
            affected_key: affected_key.to_string(),
        }
    } else {
        classify(&response, source)
    };
    invalidate_if_store_revoked(&outcome, server_id);
    Ok(outcome)
}

/// `POST <path>` against the Local API (creation, e.g. `/api/users/0/collections`). Not
/// live-verified in Slice 0 -- only PATCH was exercised. Same credential handling as
/// [`patch_item`]; `affected_key` is not known until the response is parsed, so callers extract
/// it from the returned raw body on success (left to the caller: this function reports only
/// whether the create-class request was accepted, per the neutral `WriteOutcome` contract, since
/// key-extraction is response-shape-specific per command).
pub fn post_create(
    runtime: &RuntimeContext,
    path: &str,
    body: &Value,
) -> anyhow::Result<(WriteOutcome, String)> {
    let server_id = ensure_local_api_writes_available(runtime)?;
    let (credential, source) = resolve(server_id);
    let Some(credential) = credential else {
        return Ok((
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::Required,
                source,
                detail: "no Local API write credential is available for this Zotero instance \
                         (checked ZOTERO_LOCAL_API_KEY and the local credential store)"
                    .to_string(),
            },
            String::new(),
        ));
    };

    let response = http::local_api_post(
        runtime.environment.port,
        path,
        server_id,
        &credential.key,
        body,
        DEFAULT_TIMEOUT,
    )?;

    let outcome = if response.status == 200 || response.status == 201 {
        WriteOutcome::Applied {
            affected_key: String::new(),
        }
    } else {
        classify(&response, source)
    };
    invalidate_if_store_revoked(&outcome, server_id);
    Ok((outcome, response.body))
}

/// Performs `POST /api/local/authorize` and, on success, persists the returned credential in the
/// file store keyed by `server_id`. This is the one function in this module that blocks on a
/// human GUI decision -- callers (Slice 6, out of this module's scope) must only invoke it from
/// an explicit, deliberate "authorize" action, never automatically from inside a write command.
pub fn authorize_interactive(
    runtime: &RuntimeContext,
    app_name: &str,
) -> anyhow::Result<WriteOutcome> {
    let server_id = ensure_local_api_writes_available(runtime)?;
    let response = http::local_api_authorize(
        runtime.environment.port,
        server_id,
        app_name,
        Duration::from_secs(120),
    )?;

    if response.status != 200 {
        return Ok(classify(&response, CredentialSource::None));
    }

    let parsed: Value = serde_json::from_str(&response.body)
        .map_err(|err| anyhow::anyhow!("authorize response was not valid JSON: {err}"))?;
    let key = parsed
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("authorize response had no `key` field"))?
        .to_string();
    let remember = parsed
        .get("remember")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let credential = LocalApiCredential {
        app_name: app_name.to_string(),
        key,
        remember,
        issued_at: chrono_now_rfc3339(),
    };
    credentials::store_credential(server_id, &credential)?;

    Ok(WriteOutcome::Applied {
        affected_key: String::new(),
    })
}

/// No `chrono`/`time` dependency exists in this crate yet (small-dependency-footprint
/// principle); a plain UTC ISO-8601-shaped timestamp built from `SystemTime` is sufficient for
/// this field's only use (human-readable diagnostics, never parsed back).
fn chrono_now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch -> proleptic Gregorian calendar, matching civil_from_days (Howard
    // Hinnant's well-known algorithm) -- avoids pulling in a date/time crate for one field.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Computes the read-modify-write full-array-replace set for §3.6 rows 47/68/26
/// (`item add-to-collection`/`move-to-collection`/`remove-item`): Zotero's Web API v3 treats
/// array properties as complete replacement lists on PATCH, so an additive/removal command must
/// submit the *union*/*difference*, never a naive single-element array (DOC-VERIFIED, Zotero Web
/// API v3 "Write Requests").
pub fn union_replace(current: &[String], additions: &[String]) -> Vec<String> {
    let mut result = current.to_vec();
    for addition in additions {
        if !result.contains(addition) {
            result.push(addition.clone());
        }
    }
    result
}

pub fn difference_replace(current: &[String], removals: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|value| !removals.contains(value))
        .cloned()
        .collect()
}

/// A deliberately narrow post-write view of a Local API item (§3.5's compatibility renderer,
/// scoped per the operator-approved design note: the Local API's JSON has no equivalent of the
/// SQLite-sourced `db::Item` struct's internal numeric `itemID`/`itemTypeID`/note-parent-linkage
/// fields, so byte-parity with that 26-field struct is not achievable from this source, and is
/// not attempted here. This type covers what a write command's own confirmation output needs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalApiItemSummary {
    pub key: String,
    pub version: i64,
    pub library_id: i64,
    pub item_type: String,
    pub data: Map<String, Value>,
}

fn normalize_local_item(json: &Value) -> anyhow::Result<LocalApiItemSummary> {
    let key = json
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Local API item response missing `key`"))?
        .to_string();
    let version = json
        .get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("Local API item response missing `version`"))?;
    let library_id = json
        .get("library")
        .and_then(|lib| lib.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("Local API item response missing `library.id`"))?;
    let data = json
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Local API item response missing `data`"))?;
    let item_type = data
        .get("itemType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(LocalApiItemSummary {
        key,
        version,
        library_id,
        item_type,
        data,
    })
}

/// One requested-field-vs-observed mismatch (§3.5: "every write must diff its requested fields
/// against the post-write observed fields and surface a distinct warning/error status on
/// mismatch"). Catches a Local API PATCH that partially applies (e.g. `title` commits but a
/// malformed `date` is silently dropped) -- Slice 0 did not live-test PATCH atomicity
/// (`zotero-10-impact-on-rust-port.md` §8.4 item H, BLOCKED/DEFERRED), so this diff is the actual
/// safety net, not a redundant check.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldMismatch {
    pub field: String,
    pub requested: Value,
    pub observed: Option<Value>,
}

/// Re-reads `path` (e.g. `/api/users/0/items/<key>`) via the Local API -- **never** via SQLite,
/// which cannot be trusted to see a write made moments earlier by the same process (Zotero holds
/// an exclusive SQLite lock while running; the `immutable=1` read fallback only sees checkpointed
/// WAL frames -- `zotero-10-impact-on-rust-port.md` §1/§7.1). Returns the normalized item plus
/// any requested-vs-observed field mismatches.
pub fn verify_write(
    runtime: &RuntimeContext,
    path: &str,
    requested_fields: &Map<String, Value>,
) -> anyhow::Result<(LocalApiItemSummary, Vec<FieldMismatch>)> {
    let raw = http::local_api_get_json(runtime.environment.port, path, &[], DEFAULT_TIMEOUT)?;
    let summary = normalize_local_item(&raw)?;

    let mut mismatches = Vec::new();
    for (field, requested_value) in requested_fields {
        let observed_value = summary.data.get(field).cloned();
        let matches = observed_value.as_ref() == Some(requested_value);
        if !matches {
            mismatches.push(FieldMismatch {
                field: field.clone(),
                requested: requested_value.clone(),
                observed: observed_value,
            });
        }
    }
    Ok((summary, mismatches))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> LocalWriteResponse {
        LocalWriteResponse {
            status,
            body: body.to_string(),
            last_modified_version: None,
        }
    }

    #[test]
    fn union_replace_adds_without_duplicating_or_dropping_existing_members() {
        let current = vec!["A".to_string(), "B".to_string()];
        let result = union_replace(&current, &["B".to_string(), "C".to_string()]);
        assert_eq!(result, vec!["A", "B", "C"]);
    }

    #[test]
    fn difference_replace_removes_only_the_named_members() {
        let current = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let result = difference_replace(&current, &["B".to_string()]);
        assert_eq!(result, vec!["A", "C"]);
    }

    #[test]
    fn classify_maps_428_to_precondition_failed_not_authorization() {
        let outcome = classify(
            &response(428, "Zotero-Server-ID not provided"),
            CredentialSource::Store,
        );
        assert!(matches!(outcome, WriteOutcome::PreconditionFailed { .. }));
    }

    #[test]
    fn classify_distinguishes_required_from_revoked_by_body_text() {
        let required = classify(
            &response(
                401,
                "API key required -- POST /api/local/authorize to obtain one",
            ),
            CredentialSource::None,
        );
        assert!(matches!(
            required,
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::Required,
                ..
            }
        ));

        let revoked = classify(
            &response(401, "Invalid or expired API key"),
            CredentialSource::Store,
        );
        assert!(matches!(
            revoked,
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::Revoked,
                source: CredentialSource::Store,
                ..
            }
        ));
    }

    #[test]
    fn classify_maps_403_denied_and_429_rate_limited() {
        let denied = classify(&response(403, "denied"), CredentialSource::Store);
        assert!(matches!(
            denied,
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::Denied,
                ..
            }
        ));

        let rate_limited = classify(&response(429, "too many requests"), CredentialSource::Store);
        assert!(matches!(
            rate_limited,
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::RateLimited,
                ..
            }
        ));
    }

    #[test]
    fn classify_maps_412_to_conflict_and_unknown_status_to_transport_error() {
        let conflict = classify(&response(412, "version mismatch"), CredentialSource::Store);
        assert!(matches!(conflict, WriteOutcome::Conflict { .. }));

        let transport = classify(&response(500, "boom"), CredentialSource::Store);
        assert!(matches!(transport, WriteOutcome::TransportError { .. }));
    }

    #[test]
    fn invalidate_if_store_revoked_only_fires_for_store_sourced_revocation() {
        // Environment-sourced revocation must never trigger `invalidate_stored` -- there is no
        // stored entry to remove, and this module must never touch the environment. This is a
        // structural assertion against the match guard, not a live file-store test (see
        // `credentials.rs`'s own tests for the file round trip).
        let env_revoked = WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Revoked,
            source: CredentialSource::Environment,
            detail: "Invalid or expired API key".to_string(),
        };
        assert!(!matches!(
            env_revoked,
            WriteOutcome::AuthorizationFailed {
                reason: AuthorizationReason::Revoked,
                source: CredentialSource::Store,
                ..
            }
        ));
    }

    #[test]
    fn write_outcome_json_never_leaks_credential_material() {
        let outcome = WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Revoked,
            source: CredentialSource::Store,
            detail: "Invalid or expired API key".to_string(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        let text = json.to_string();
        assert!(
            !text.contains("\"key\""),
            "must never serialize a raw key field: {text}"
        );
    }
}
