from __future__ import annotations

import argparse
import csv
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

from fixtures.build_fixture import REFERENCE_ROOT, build_fixture
from normalize import normalize_capture


REPO_ROOT = Path(__file__).resolve().parents[1]
COMMANDS_PATH = REPO_ROOT / "harness" / "commands.tsv"
GOLDEN_ROOT = REPO_ROOT / "harness" / "golden"


def _import_fake_server():
    ref = str(REFERENCE_ROOT)
    if ref not in sys.path:
        sys.path.insert(0, ref)
    from cli_anything.zotero.tests._helpers import fake_zotero_http_server

    return fake_zotero_http_server


def load_commands(path: Path = COMMANDS_PATH) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t"))


def command_slug(command: str) -> str:
    return command.replace(" ", "__").replace("/", "_")


def substitute_args(args: list[str], paths: dict[str, str], output_dir: Path) -> list[str]:
    replacements = {
        "{item}": "REG12345",
        "{group_item}": "GROUPKEY",
        "{collection}": "COLLAAAA",
        "{group_collection}": "GCOLLAAA",
        "{tag}": "sample-tag",
        "{search}": "SEARCHKEY",
        "{query}": "Sample",
        "{unicode_query}": "中文",
        "{doi}": "10.1000/sample",
        "{pmid}": "12345678",
        "{arxiv}": "2602.02093",
        "{url}": "https://example.com/paper",
        "{bib}": str(Path(paths["input_dir"]) / "sample.bib"),
        "{ris}": str(Path(paths["input_dir"]) / "sample.ris"),
        "{json}": str(Path(paths["input_dir"]) / "sample.json"),
        "{pdf}": str(Path(paths["input_dir"]) / "sample.pdf"),
        "{docx_placeholders}": str(Path(paths["input_dir"]) / "placeholders.docx"),
        "{docx_citations}": str(Path(paths["input_dir"]) / "citations.docx"),
        "{out_bib}": str(output_dir / "out.bib"),
        "{out_docx}": str(output_dir / "out.docx"),
    }
    return [replacements.get(arg, arg) for arg in args]


def resolve_impl(value: str) -> list[str]:
    if value == "python":
        return [sys.executable, "-m", "cli_anything.zotero"]
    return [value]


def run_one(row: dict[str, str], *, impl: list[str], output_root: Path) -> dict[str, Any]:
    args = json.loads(row["args_json"])
    state = row["fixture_state"]
    if state == "live-only":
        return {
            "command": row["command"],
            "compatibility_class": row["class"],
            "fixture_state": state,
            "skipped": True,
            "reason": row.get("notes", "live-only"),
        }

    fake_zotero_http_server = _import_fake_server()
    with tempfile.TemporaryDirectory(prefix="zotero-harness-") as tmp:
        tmp_path = Path(tmp)
        paths = build_fixture(tmp_path / "fixture", state)
        local_api_status = 403 if state == "local-api-off" else 200
        with fake_zotero_http_server(
            local_api_root_status=local_api_status,
            sqlite_path=paths["sqlite_path"],
            data_dir=paths["data_dir"],
        ) as server:
            env = os.environ.copy()
            env["PYTHONPATH"] = str(REFERENCE_ROOT) + os.pathsep + env.get("PYTHONPATH", "")
            env["ZOTERO_PROFILE_DIR"] = paths["profile_dir"]
            env["ZOTERO_DATA_DIR"] = paths["data_dir"]
            env["ZOTERO_EXECUTABLE"] = paths["executable"]
            env["ZOTERO_HTTP_PORT"] = str(server["port"])
            env["CLI_ANYTHING_ZOTERO_STATE_DIR"] = str(tmp_path / "state")
            env["ZOTERO_CLI_AUDIT_DIR"] = str(tmp_path / "audit")
            env["ZOTERO_VECTOR_DB"] = str(tmp_path / "vectors.sqlite")
            env["CLI_ANYTHING_ZOTERO_OPENAI_URL"] = f"http://127.0.0.1:{server['port']}/v1/responses"
            env.setdefault("OPENAI_API_KEY", "test-key")

            output_dir = tmp_path / "outputs"
            output_dir.mkdir()
            full_args = substitute_args(args, paths, output_dir)
            proc = subprocess.run(impl + full_args, cwd=REFERENCE_ROOT, env=env, text=True, capture_output=True)

            capture = {
                "command": row["command"],
                "args": full_args,
                "compatibility_class": row["class"],
                "fixture_state": state,
                "fixture_root": paths["fixture_root"],
                "_normalization_roots": [str(tmp_path), paths["fixture_root"]],
                "exit_code": proc.returncode,
                "stdout": proc.stdout,
                "stderr": proc.stderr,
                "http_calls": server["calls"],
            }
            return normalize_capture(capture)


def capture_all(*, impl: list[str], output_root: Path, command_filter: str | None = None) -> dict[str, Any]:
    rows = load_commands()
    if command_filter:
        rows = [row for row in rows if row["command"] == command_filter]
    output_root.mkdir(parents=True, exist_ok=True)
    summary = {"captured": 0, "skipped": 0, "failed": 0, "commands": []}
    for row in rows:
        result = run_one(row, impl=impl, output_root=output_root)
        slug = command_slug(row["command"])
        target = output_root / f"{slug}.json"
        target.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if result.get("skipped"):
            summary["skipped"] += 1
        elif "Traceback (most recent call last):" in str(result.get("stderr", "")):
            summary["failed"] += 1
        else:
            summary["captured"] += 1
        summary["commands"].append({"command": row["command"], "file": str(target), "skipped": bool(result.get("skipped")), "exit_code": result.get("exit_code")})
    return summary


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--impl", default="python", help="'python' or path to implementation binary")
    parser.add_argument("--output", type=Path, default=GOLDEN_ROOT / "python")
    parser.add_argument("--command")
    parser.add_argument("--clean", action="store_true")
    args = parser.parse_args()

    if args.clean and args.output.exists():
        shutil.rmtree(args.output)
    summary = capture_all(impl=resolve_impl(args.impl), output_root=args.output, command_filter=args.command)
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
