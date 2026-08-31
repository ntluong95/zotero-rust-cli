# Security model

What `zotero-cli` can touch, what it deliberately cannot, and where the real
exposure is. Read this before running it on a shared machine or wiring it into an
autonomous agent.

## Reporting a vulnerability

Open a [security advisory](https://github.com/ntluong95/zotero-rust-cli/security/advisories/new)
on the repository rather than a public issue.

---

## 1. The CLI Bridge eval endpoint — the main exposure

The CLI Bridge is a Zotero plugin that registers **`POST /cli-bridge/eval`** on
Zotero's own built-in HTTP server. The CLI posts JavaScript there; Zotero runs it
with full privileges inside the application.

**This is the most powerful surface in the system.** Anything that can reach that
endpoint can do anything Zotero can do: read the entire library, modify it,
delete it, and read files the Zotero process can read.

What constrains it:

| Constraint | Detail |
|---|---|
| Loopback only | The CLI connects to `http://127.0.0.1:<port>/cli-bridge/eval`. Zotero's HTTP server is a local server; it is not exposed to the network by this project. |
| POST only | The endpoint declares `supportedMethods: ["POST"]`. |
| No bookmarklet access | `permitBookmarklet: false` — a web page cannot drive it through Zotero's bookmarklet path. |
| Zotero 10 header hardening | Zotero 10 itself rejects requests carrying an `Origin` header or a `Mozilla/`-prefixed `User-Agent`, and enforces a `Host` allowlist. This is Zotero's protection against browser-originated requests, verified live and pinned by a regression test (`tests/zotero10_conformance.rs`). |
| Opt-in | The Bridge only exists if a human installed the plugin through Zotero's own Add-ons dialog. The CLI stages the XPI; it never writes into the Zotero profile and never bypasses plugin consent. |
| Ownership verification | The fork's Bridge answers an ownership probe identifying itself (`fork: "zotero-rust-cli"`, addon id `cli-bridge@cli-anything-rust.dev`). An HTTP 200 alone is not accepted — the CLI verifies ownership before trusting the endpoint, so the upstream Python project's bridge cannot be mistaken for this one. |

**What this does not protect against:** any other local process running as your
user. Loopback is not an authentication boundary. If you do not trust every
process on the machine with your Zotero library, do not install the Bridge.

### `zotero-cli js`

`js` is the direct, unmediated path to that endpoint — arbitrary privileged
JavaScript, by design. It exists for expert debugging.

It is **not** a fallback for a failed typed command. Using it that way bypasses
target validation, write-backend routing, result verification, and the audit log.
Agents driving this CLI are instructed (see [`AGENTS.md`](AGENTS.md)) never to
substitute `js` for a typed write, and to report a typed-write failure on a
`ready` environment as a bug instead.

### JS injection

Bridge JavaScript is built by serializing parameters with `serde_json` and
passing them through `JSON.parse` — not by string concatenation. The upstream
Python implementation concatenated strings while escaping only `'`; that defect
was fixed rather than ported. Semantic-search SQL likewise uses bound parameters
instead of f-string interpolation.

---

## 2. Local API write authorization

On Zotero 10+, writes can route through Zotero's Local API instead of the Bridge.
That path is gated on a **human consent dialog inside Zotero**:

```bash
zotero-cli app authorize-local-api
```

- The CLI performs the real `POST /api/local/authorize` handshake and blocks on
  the dialog. It cannot approve on your behalf and never attempts to.
- Without approval, writes stop with `authorization_failed` /
  `needs_human_action`. They do not silently fall through to some other path.
- Automatically launching Zotero is **not** consent. Those are separate gates,
  and starting the app grants nothing.

### Credential storage

Zotero re-prompts for consent on every `authorize` call, even with an existing
"Always Allow" grant — so a stateless CLI has to persist the key itself to write
unattended. Two sources, checked in order, never mixed:

1. **`ZOTERO_LOCAL_API_KEY`** — operator-owned. The CLI reads it and never
   writes, modifies, or deletes it. On rejection it only reports the failure.
2. **A CLI-owned file store**, scoped to a specific `Zotero-Server-ID`, stored
   beside `session.json` (`CLI_ANYTHING_ZOTERO_STATE_DIR`) as a **separate file**
   with restrictive permissions, replaced atomically.

Threat model: the same as an SSH private key or `~/.netrc` — a
restrictive-permission local file, not an OS keychain. A keychain backend is out
of v1 scope.

The credential type's `Debug`/`Display` implementations never print the key, only
its byte length, so an accidental `{:?}` or log line cannot leak it. A standing
test (`tests/write_output_denylist.rs`) asserts that no write command leaks
backend identity, `server_id`, or internal versioning into stdout JSON.

A stored entry is not assumed valid forever: the CLI also handles the server's
own 401 rejection.

---

## 3. Database safety

- **The CLI never writes to `zotero.sqlite` directly.** Every write goes through
  Zotero, via the Local API or the Bridge. The upstream `--experimental`
  direct-SQLite write path was removed, not ported.
- **Reads are opened read-only.** On Zotero 10+ (WAL mode), when Zotero holds the
  database lock and a consistent read is impossible, the CLI **refuses loudly**
  rather than falling back to `immutable=1`, which would silently drop every
  committed-but-uncheckpointed row with exit code 0 and no error. There is no
  bypass flag for that refusal.
- `item find` and `library list` route around that state by using an
  already-running, ownership-verified Bridge to run Zotero's own read. They never
  autolaunch Zotero to do it.

Full evidence: [`ZOTERO-COMPATIBILITY.md`](ZOTERO-COMPATIBILITY.md).

---

## 4. Outbound data flows

Two command families send your library content to a network endpoint you
configure. Both are opt-in by virtue of being separate commands, but an agent
driving the CLI should surface them to the user explicitly.

| Command | Sends | Endpoint | Credential |
|---|---|---|---|
| `item build-index` | Item titles, abstracts, and note/attachment text, chunked | `ZOTERO_EMBED_API`, default `http://127.0.0.1:8080/v1/embeddings` (loopback) | `ZOTERO_EMBED_KEY` |
| `item analyze` | The item's assembled LLM context (metadata, and optionally notes/BibTeX/CSL-JSON) | `CLI_ANYTHING_ZOTERO_OPENAI_URL`, default `https://api.openai.com/v1/chat/completions` | `OPENAI_API_KEY` |

Notes:

- The embedding default is **loopback**, so out of the box `build-index` talks to
  a local model server and nothing leaves the machine. Repointing
  `ZOTERO_EMBED_API` at a remote host is an operator decision.
- The analysis default is **OpenAI's public API**, so `item analyze` sends item
  content to a third party unless you repoint it. `item analyze` refuses to run
  without `OPENAI_API_KEY` and suggests `item context` for model-independent
  output instead.
- The CLI does **not** enforce a URL scheme on either variable. If you configure a
  plaintext `http://` endpoint on a non-loopback host, the content travels in
  clear. That is your configuration to get right.
- Neither command prints the configured API key.

---

## 5. Audit log

Every write records an entry in a JSONL audit log:

```bash
zotero-cli audit path          # where it lives
zotero-cli audit tail --limit 20
```

Location is overridable with `ZOTERO_CLI_AUDIT_DIR`. The format is shared with
the upstream Python implementation, so entries from both interleave harmlessly.

This is the record to show a user after an agent has mutated their library. It is
a local log for accountability, not a tamper-proof journal — anything running as
your user can edit it.

---

## 6. Process launching

Commands needing a live backend will start Zotero themselves, at most once, only
when it appears closed, using the discovered or explicitly configured
(`ZOTERO_EXECUTABLE`) executable. Diagnostics and offline-capable reads never
start anything.

Disable it entirely on headless or shared systems:

```bash
export ZOTERO_CLI_NO_AUTOLAUNCH=1
```

---

## 7. Supply chain

- Release binaries are built in GitHub Actions from a pinned Rust toolchain
  across five targets, and published with **GitHub Artifact Attestations**
  (Sigstore-backed, keyless). Verify before trusting a download:

  ```bash
  gh attestation verify <archive> --repo ntluong95/zotero-rust-cli
  ```

- `SHA256SUMS` is published alongside every release.
- macOS binaries are ad-hoc signed. That satisfies Apple Silicon's
  "must be signed" requirement but does **not** clear Gatekeeper quarantine on a
  browser/`curl` download — see [`INSTALL.md`](INSTALL.md#macos-quarantine-explained).
- Every dependency's license is enumerated in `THIRD-PARTY-LICENSES.md`,
  regenerated per release; an unrecognized license in the dependency tree fails
  the release build rather than shipping silently.
