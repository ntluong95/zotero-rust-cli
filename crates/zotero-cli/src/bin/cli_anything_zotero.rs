// Alias binary for the `cli-anything-zotero` entrypoint. Upstream wires
// both console_scripts to the exact same entrypoint function (setup.py),
// so this stays behaviourally identical to `zotero-cli`.
fn main() {
    std::process::exit(zotero_cli::run());
}
