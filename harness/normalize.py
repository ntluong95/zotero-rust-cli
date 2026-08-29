from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


TIMESTAMP_RE = re.compile(r"\b20\d\d-\d\d-\d\d(?:[T ][0-9:.+-]+)?\b")
ZOTERO_KEY_RE = re.compile(r"\b[23456789A-Z]{8}\b")
PID_RE = re.compile(r'"pid"\s*:\s*\d+')
PORT_RE = re.compile(r'("port"\s*:\s*)\d+')
SESSION_ID_RE = re.compile(r'import-(?:json|file)-[0-9a-f]{32}')

# Accepted divergence, scoped to the `zotero-unreachable` fixture only
# (see plans/reports/compatibility-matrix.md): Python's `urllib` and
# Rust's `ureq` produce OS- and library-specific connection-refused
# prose that cannot be made byte-identical (and chasing it would mean
# imitating one transport library's exception text in the other
# language, which is not a real compatibility contract). Everything
# else about the fixture -- JSON structure, the false/false
# reachability booleans, and the exit code -- stays a real comparison.
# This substitution is applied only for that one fixture_state and
# only to these two named JSON fields; it must never be widened to a
# blanket "normalize all error text" rule.
CONNECTOR_MESSAGE_RE = re.compile(r'("connector_message"\s*:\s*)"(?:[^"\\]|\\.)*"')
LOCAL_API_MESSAGE_RE = re.compile(r'("local_api_message"\s*:\s*)"(?:[^"\\]|\\.)*"')


def normalize_unreachable_transport_messages(text: str) -> str:
    text = CONNECTOR_MESSAGE_RE.sub(r'\1"<CONNECTION_REFUSED>"', text)
    text = LOCAL_API_MESSAGE_RE.sub(r'\1"<CONNECTION_REFUSED>"', text)
    return text


def normalize_text(text: str, *, roots: list[str] | None = None) -> str:
    out = text.replace("\r\n", "\n")
    for root in roots or []:
        if root.startswith("/var/"):
            private_root = "/private" + root
            out = out.replace(private_root, "<ROOT>")
        if root.startswith("/private/var/"):
            public_root = root.removeprefix("/private")
            out = out.replace(public_root, "<ROOT>")
        out = out.replace(root, "<ROOT>")
        out = out.replace(root.replace("/", "\\"), "<ROOT>")
    home = str(Path.home())
    out = out.replace(home, "<HOME>")
    out = out.replace(home.replace("/", "\\"), "<HOME>")
    out = TIMESTAMP_RE.sub("<TIMESTAMP>", out)
    out = PID_RE.sub('"pid": "<PID>"', out)
    out = PORT_RE.sub(r'\1"<PORT>"', out)
    out = SESSION_ID_RE.sub("<SESSION_ID>", out)
    out = out.replace("<ROOT>\\", "<ROOT>/")
    out = out.replace("<HOME>\\", "<HOME>/")
    out = out.replace("/private<ROOT>", "<ROOT>")
    return out


def normalize_value(value: Any, *, roots: list[str]) -> Any:
    if isinstance(value, str):
        return normalize_text(value, roots=roots)
    if isinstance(value, list):
        return [normalize_value(item, roots=roots) for item in value]
    if isinstance(value, dict):
        return {key: normalize_value(item, roots=roots) for key, item in value.items()}
    return value


def normalize_capture(capture: dict[str, Any]) -> dict[str, Any]:
    roots = [str(root) for root in capture.get("_normalization_roots", []) if root]
    fixture_root = capture.get("fixture_root")
    if fixture_root and str(fixture_root) not in roots:
        roots.append(str(fixture_root))
    normalized = normalize_value(capture, roots=roots)
    normalized.pop("_normalization_roots", None)
    if normalized.get("fixture_state") == "zotero-unreachable":
        for field in ("stdout", "stderr"):
            value = normalized.get(field)
            if isinstance(value, str):
                normalized[field] = normalize_unreachable_transport_messages(value)
    return normalized


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    data = json.loads(args.input.read_text(encoding="utf-8"))
    normalized = normalize_capture(data)
    text = json.dumps(normalized, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
