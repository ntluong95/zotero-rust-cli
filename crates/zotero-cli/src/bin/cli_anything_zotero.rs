fn main() {
    // Alias binary for the `cli-anything-zotero` entrypoint. Upstream wires
    // both console_scripts to the exact same entrypoint function, so this
    // stays behaviourally identical to `zotero-cli` (see setup.py).
    println!("cli-anything-zotero {}", env!("CARGO_PKG_VERSION"));
}
