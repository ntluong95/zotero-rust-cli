# RC2 REAL-ZOTERO SMOKE TEST — human checklist

Automated tests cannot prove this candidate. Every automated test runs against a mock HTTP
server with `ZOTERO_CLI_NO_AUTOLAUNCH=1` set, so no automated run has ever started or mutated a
real Zotero. The three fixed defects were all runtime-orchestration problems that an isolated
suite structurally could not see — which is exactly why this checklist exists.

**RC2 is not ready until this passes.**

---

## 0. Before you start

Use a **disposable or non-precious item**. Do not run the destructive steps against a library
item you care about. Step 1 creates one for you.

Build and put the candidate on PATH:

```bash
cargo build --release --bin zotero-cli
```

The binary is at `target/release/zotero-cli`. Either add that directory to `PATH` for this shell
or call it by full path. Confirm you are testing the candidate, not the installed RC1:

```bash
zotero-cli --version
```

Expected: `zotero-cli 1.0.0-rc.2`. If it prints `1.0.0-rc.1` you are running the old binary.

---

## 1. Create a disposable test item (Zotero open)

With Zotero running, create a throwaway item and note its key:

```bash
zotero-cli --json js "var i = new Zotero.Item('document'); i.setField('title', 'RC2 SMOKE TEST — safe to delete'); i.libraryID = Zotero.Libraries.userLibraryID; return {key: i.key, id: i.saveTx()};"
```

Record the `key` it prints as `<TEST_KEY>` for the rest of this checklist. (This is the one
place raw `js` is the right tool: creating a scratch fixture, not working around a failure.)

Also confirm your session is not scoped to some other library, which would make the commands
below target the wrong place:

```bash
zotero-cli --json session status
```

If `current_library` is set to a library other than your personal one, clear it before
continuing (`zotero-cli session use-library 1`) or expect "not found" results.

---

## 2. THE MAIN TEST — auto-launch + typed write, starting from a closed Zotero

**Quit Zotero completely.** Not minimized — fully quit. Verify:

```bash
zotero-cli --json app doctor
```

Expected (**Problem C / CASE J**):
- exit code 1, `ready: false`
- `checks.connector.ok: false`
- `checks.bridge.state: "installed_zotero_closed"` (the plugin is installed; Zotero is not up)
- **Zotero does not start.** Diagnostics observe, they never launch.

Now run the typed write with Zotero still closed:

```bash
zotero-cli --json note add <TEST_KEY> --text "RC2 smoke test note"
```

Expected (**Problems A + C**):
- Zotero starts **by itself** — exactly one Zotero window, not two.
- The command waits for the CLI Bridge, then completes.
- Exit code 0, and JSON with `"action": "note_add"`, a `key`, and
  `"parentItemKey": "<TEST_KEY>"`.
- **No** `app launch` → wait → retry choreography was needed.
- **No** SQLite lock/WAL error anywhere in the output. (On RC1 this command failed here with
  "Zotero appears to be running and holds an exclusive lock on the WAL-mode database…".)

If it instead reports a readiness timeout, note how long Zotero took to start and retry with
`ZOTERO_CLI_LAUNCH_TIMEOUT=120`.

---

## 3. Verify the note actually exists

With Zotero now running, read it back through a normal command:

```bash
zotero-cli --json item notes <TEST_KEY>
```

Expected: the new note appears, with the text from step 2.

Independently confirm through Zotero's own UI that the note is attached to the test item.

---

## 4. `app doctor` and `js` must agree (Problem B)

Run these **back to back**, in this order, with Zotero running:

```bash
zotero-cli --json app doctor
```

```bash
zotero-cli --json js "return {agree: true, version: Zotero.version};"
```

Expected:
- `doctor` reports `checks.bridge.ok: true`, `checks.bridge.state: "healthy"`, and a
  `checks.bridge.port`.
- `js` **succeeds** and prints `{"agree": true, "version": "…"}`.
- They must not contradict each other. On RC1, `doctor` reported a healthy Bridge while `js`
  in the same second reported "JS Bridge endpoint not available", because `doctor` used the
  profile's configured port and every other Bridge caller hard-coded 23119.

Cross-check the port `doctor` reports against your profile:

```bash
grep httpServer.port ~/Library/Application\ Support/Zotero/Profiles/*/prefs.js
```

(On Linux: `~/.zotero/zotero/*/prefs.js`. On Windows:
`%APPDATA%\Zotero\Zotero\Profiles\*\prefs.js`.) If a port is set there, `doctor` must report
that port — not 23119.

---

## 5. No second Zotero (CASE F)

With Zotero already running:

```bash
zotero-cli --json js "return 1 + 1;"
```

Expected: returns `2` promptly, and **no new Zotero process or window appears.**

---

## 6. Offline reads still work with Zotero closed (CASE I)

**Quit Zotero again.** Then:

```bash
zotero-cli --json item get <TEST_KEY>
```

Expected: succeeds from the local database, and **does not start Zotero**.

---

## 7. Safety invariants — these must all still hold

With Zotero running:

```bash
zotero-cli --json item merge <TEST_KEY> SOMEOTHERKEY
```

Expected: `"status": "dry_run"`, `"dry_run": true`, **no mutation**. A bare `item merge` must
never merge; `--confirm` is required. (If your session is scoped to a different library this
may report "not found" instead — that is scoping, not a merge.)

```bash
zotero-cli --json audit tail --limit 5
```

Expected: the write from step 2 is recorded, and **no API key, token, or `Authorization`
header value appears anywhere** in the audit output.

Optional authorization check (**do not approve anything you did not intend to**): if you have
not authorized Local API writes on this instance, a Local-API-routed write must stop with
`"outcome": "authorization_failed"`, `"needs_human_action": true` — never an automatic consent
grant, and never a silent fallback.

---

## 8. Clean up

Delete the disposable item created in step 1:

```bash
zotero-cli --json item delete <TEST_KEY> --confirm
```

Then verify it is gone:

```bash
zotero-cli --json item get <TEST_KEY>
```

Expected: not found.

---

## Result

| # | Check | Pass? |
|---|-------|-------|
| 2a | `doctor` with Zotero closed reports honestly and starts nothing | |
| 2b | `note add` auto-launches Zotero exactly once and succeeds | |
| 2c | No SQLite lock/WAL error during the write | |
| 3 | Note is readable afterwards and visible in Zotero | |
| 4 | `doctor` and `js` agree; reported port matches the profile pref | |
| 5 | No second Zotero started when one is already running | |
| 6 | Offline read works with Zotero closed, starts nothing | |
| 7a | Bare `item merge` is still a dry run | |
| 7b | No credential material in audit output | |
| 8 | Cleanup succeeds | |

If any row fails, RC2 is **not** ready — capture the exact command, full JSON output, and
whether Zotero was open or closed at the time.
