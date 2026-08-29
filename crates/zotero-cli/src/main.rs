fn main() {
    // Phase 2 distribution-pipeline stub. Exact --version output format
    // (matching upstream's `cli-anything-zotero, version X.Y.Z`) lands in
    // Phase 3 alongside the rest of the result contract.
    println!("zotero-cli {}", env!("CARGO_PKG_VERSION"));
}
