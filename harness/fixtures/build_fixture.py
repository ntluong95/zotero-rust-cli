from __future__ import annotations

import argparse
import json
import multiprocessing
import os
import sqlite3
import zipfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
REFERENCE_ROOT = REPO_ROOT / "reference" / "cli-anything-zotero"


def _import_helpers():
    import sys

    ref = str(REFERENCE_ROOT)
    if ref not in sys.path:
        sys.path.insert(0, ref)
    from cli_anything.zotero.tests._helpers import create_sample_environment

    return create_sample_environment


def _write_pref(profile_dir: Path, *, local_api_enabled: bool, data_dir: Path) -> None:
    value = str(data_dir).replace("\\", "\\\\")
    prefs = (
        'user_pref("extensions.zotero.useDataDir", true);\n'
        f'user_pref("extensions.zotero.dataDir", "{value}");\n'
        'user_pref("extensions.zotero.httpServer.port", 23119);\n'
        f'user_pref("extensions.zotero.httpServer.localAPI.enabled", {str(local_api_enabled).lower()});\n'
    )
    (profile_dir / "prefs.js").write_text(prefs, encoding="utf-8")


def _add_unicode_case(sqlite_path: Path) -> None:
    with sqlite3.connect(sqlite_path) as conn:
        cur = conn.cursor()
        cur.execute(
            "INSERT INTO items VALUES (?, ?, '2026-01-03', '2026-01-03', '2026-01-03', ?, ?, 1, 1)",
            (90, 1, 1, "UNICODE1"),
        )
        cur.execute("INSERT INTO itemDataValues VALUES (?, ?)", (90, "C:\\Users\\x 'quoted' \"double\"\n<script> 中文"))
        cur.execute("INSERT INTO itemData VALUES (?, ?, ?)", (90, 1, 90))
        cur.execute("INSERT INTO tags VALUES (?, ?)", (90, "tag\\slash 'quote' 中文"))
        cur.execute("INSERT INTO itemTags VALUES (?, ?, 0)", (90, 90))
        cur.execute(
            "INSERT INTO collections VALUES (?, ?, ?, '2026-01-03', ?, ?, 1, 1)",
            (90, "Collection\\newline\n<script> 中文", None, 1, "UNICOLL1"),
        )
        cur.execute("INSERT INTO collectionItems VALUES (?, ?, 0)", (90, 90))


def _wal_worker(sqlite_path: str) -> None:
    conn = sqlite3.connect(sqlite_path)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute(
        "INSERT INTO items VALUES (?, ?, '2026-08-17', '2026-08-17', '2026-08-17', ?, ?, 1, 1)",
        (91, 1, 1, "WALFIX01"),
    )
    conn.commit()
    # Deliberately never call conn.close(): SQLite checkpoints a WAL
    # database's -wal file back into the main file as part of closing the
    # *last* connection to it (see sqlite.org "Checkpoint Starvation" /
    # "Closing WAL" docs) -- a clean close here would checkpoint this row
    # right back out of -wal, defeating the entire point of this fixture
    # (phase-14-zotero-10-compatibility-gate.md §2: "The fixture must leave
    # rows in the WAL, uncheckpointed -- a fixture that checkpoints on
    # close proves nothing"). `os._exit()` terminates this child process
    # immediately, skipping Python's interpreter shutdown/GC/atexit
    # entirely, so `sqlite3_close()` (and its checkpoint-on-last-close
    # logic) never runs. This is portable: unlike sending a kill signal to
    # simulate a crash, `os._exit()` works identically on Windows, macOS,
    # and Linux.
    os._exit(0)


def _add_wal_mode_case(sqlite_path: Path) -> None:
    # Runs in a genuinely separate process (not just a thread) so this
    # process's own `sqlite3` connection -- and Python's normal interpreter
    # shutdown, which would otherwise run on this process's exit -- can
    # never touch the child's connection or trigger its checkpoint.
    proc = multiprocessing.Process(target=_wal_worker, args=(str(sqlite_path),))
    proc.start()
    proc.join(timeout=30)
    if proc.is_alive():
        proc.terminate()
        proc.join()
    wal_path = sqlite_path.with_name(sqlite_path.name + "-wal")
    if not wal_path.exists() or wal_path.stat().st_size == 0:
        raise RuntimeError(
            "wal-mode fixture setup did not leave an uncheckpointed -wal "
            "file; this fixture cannot demonstrate the bug it exists to "
            "cover -- see phase-14-zotero-10-compatibility-gate.md §2"
        )


def _empty_library(sqlite_path: Path) -> None:
    with sqlite3.connect(sqlite_path) as conn:
        for table in (
            "collectionItems",
            "itemTags",
            "itemAnnotations",
            "itemAttachments",
            "itemNotes",
            "itemCreators",
            "itemData",
            "itemDataValues",
            "items",
            "collections",
            "savedSearchConditions",
            "savedSearches",
            "tags",
        ):
            conn.execute(f"DELETE FROM {table}")


def _write_docx(path: Path, document_xml_body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    document_xml = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>{document_xml_body}<w:sectPr/></w:body>
</w:document>
"""
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
        zf.writestr("_rels/.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
        zf.writestr("word/document.xml", document_xml)


def build_fixture(base: Path, state: str) -> dict[str, str]:
    create_sample_environment = _import_helpers()
    base.mkdir(parents=True, exist_ok=True)
    paths = create_sample_environment(base)

    local_api_enabled = state != "local-api-off"
    _write_pref(paths["profile_dir"], local_api_enabled=local_api_enabled, data_dir=paths["data_dir"])

    if state == "empty-library":
        _empty_library(paths["sqlite_path"])
    if state == "unicode-cjk":
        _add_unicode_case(paths["sqlite_path"])
    if state == "wal-mode":
        _add_wal_mode_case(paths["sqlite_path"])

    input_dir = base / "inputs"
    input_dir.mkdir(exist_ok=True)
    (input_dir / "sample.bib").write_text("@article{fixture,title={Fixture BibTeX}}\n", encoding="utf-8")
    (input_dir / "sample.ris").write_text("TY  - JOUR\nTI  - Fixture RIS\nER  - \n", encoding="utf-8")
    (input_dir / "sample.json").write_text(
        json.dumps([{"itemType": "journalArticle", "title": "Fixture JSON"}], ensure_ascii=False),
        encoding="utf-8",
    )
    (input_dir / "sample.pdf").write_bytes(b"%PDF-1.4\n" + b"x" * 9000)
    _write_docx(input_dir / "placeholders.docx", "<w:p><w:r><w:t>Known {{zotero:REG12345}} and missing {{zotero:NOITEM99}}.</w:t></w:r></w:p>")
    _write_docx(input_dir / "citations.docx", '<w:p><w:r><w:instrText xml:space="preserve"> ADDIN EN.CITE </w:instrText></w:r><w:r><w:t>[1]</w:t></w:r></w:p>')

    result = {key: str(value) for key, value in paths.items()}
    result["fixture_root"] = str(base)
    result["input_dir"] = str(input_dir)
    result["state"] = state
    result["local_api_enabled"] = str(local_api_enabled).lower()
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=Path)
    parser.add_argument(
        "--state",
        default="local-api-on",
        choices=[
            "local-api-on",
            "local-api-off",
            "empty-library",
            "group-library",
            "unicode-cjk",
            # Zotero-10-compatibility-gate fixture
            # (phase-14-zotero-10-compatibility-gate.md §2): leaves item
            # WALFIX01 (id 91) committed to the WAL journal but
            # deliberately uncheckpointed, so a reader that ignores -wal
            # (the pre-fix `mode=ro&immutable=1` connection) observably
            # misses it while a WAL-aware reader (`mode=ro`) sees it.
            "wal-mode",
            # No dedicated data mutation (see build_fixture()) -- this
            # state only changes capture.py's control flow (no fake
            # HTTP server is started at all). Listed here so `--state`
            # discovery/`--help` stays a complete inventory.
            "zotero-unreachable",
        ],
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    paths = build_fixture(args.output_dir, args.state)
    if args.json:
        print(json.dumps(paths, ensure_ascii=False, indent=2))
    else:
        print(paths["fixture_root"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
