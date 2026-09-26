from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path

COMMANDS_PATH = Path(__file__).resolve().parent / "commands.tsv"


def load_classes(path: Path = COMMANDS_PATH) -> dict[str, str]:
    """The *current* compatibility class per command label.

    Captures record the class that was current when they were taken, so a golden file captured
    before a row was reclassified still carries the old class. Classification therefore reads
    the live commands.tsv rather than trusting either capture.
    """
    if not path.exists():
        return {}
    with path.open(encoding="utf-8", newline="") as fh:
        return {row["command"]: row["class"] for row in csv.DictReader(fh, delimiter="\t")}


def _looks_json(text: str) -> bool:
    text = text.strip()
    if not text:
        return True
    if text[0] not in "[{":
        return False
    try:
        json.loads(text)
    except json.JSONDecodeError:
        return False
    return True


def classify_capture(expected: dict, actual: dict, current_class: str | None = None) -> str:
    if expected.get("skipped") or actual.get("skipped"):
        return "Skipped" if expected == actual else "Mismatch"
    if expected == actual:
        return "Exact"
    if expected.get("command") != actual.get("command"):
        return "Mismatch"
    if current_class == "Changed":
        # A documented, intentional divergence (see the row's notes for the Rust test that owns
        # the behavior). It still must not crash or emit non-JSON stdout.
        if "Traceback (most recent call last):" in str(actual.get("stderr", "")):
            return "Mismatch"
        if not _looks_json(str(actual.get("stdout", ""))):
            return "Mismatch"
        return "Changed"
    if expected.get("compatibility_class") != actual.get("compatibility_class"):
        return "Mismatch"
    if expected.get("fixture_state") != actual.get("fixture_state"):
        return "Mismatch"
    if expected.get("exit_code") != actual.get("exit_code"):
        return "Mismatch"
    if "Traceback (most recent call last):" in str(actual.get("stderr", "")):
        return "Mismatch"
    if not _looks_json(str(actual.get("stdout", ""))):
        return "Mismatch"
    return "Semantic" if expected.get("compatibility_class") == "Semantic" else "Mismatch"


def compare_dirs(expected: Path, actual: Path) -> dict:
    expected_files = {path.name: path for path in expected.glob("*.json")}
    actual_files = {path.name: path for path in actual.glob("*.json")}
    names = sorted(set(expected_files) | set(actual_files))
    rows = []
    classes = load_classes()
    summary = {
        "exact": 0,
        "semantic": 0,
        "changed": 0,
        "skipped": 0,
        "mismatch": 0,
        "missing": 0,
    }
    for name in names:
        if name not in expected_files or name not in actual_files:
            rows.append({"file": name, "status": "missing"})
            summary["missing"] += 1
            continue
        left = json.loads(expected_files[name].read_text(encoding="utf-8"))
        right = json.loads(actual_files[name].read_text(encoding="utf-8"))
        status = classify_capture(left, right, classes.get(left.get("command")))
        summary[status.lower()] += 1
        rows.append({"file": name, "command": left.get("command"), "status": status})
    summary["total"] = len(names)
    return {"summary": summary, "results": rows}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("expected", type=Path)
    parser.add_argument("actual", type=Path)
    args = parser.parse_args()

    result = compare_dirs(args.expected, args.actual)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 1 if result["summary"]["mismatch"] or result["summary"]["missing"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
