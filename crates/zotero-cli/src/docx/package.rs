//! OPC / DOCX ZIP package handling.
//!
//! Provides validation of DOCX paths, byte-for-byte read/write of OPC ZIP parts,
//! and preservation of unmodified archive entries.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// Validates that a DOCX path exists and has a `.docx` extension.
pub fn validate_docx_path<P: AsRef<Path>>(path: P) -> anyhow::Result<PathBuf> {
    let p = path.as_ref();
    let expanded = if let Ok(stripped) = p.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            home.join(stripped)
        } else {
            p.to_path_buf()
        }
    } else {
        p.to_path_buf()
    };

    if !expanded.exists() {
        anyhow::bail!("DOCX file not found: {}", expanded.display());
    }

    let is_docx = expanded
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("docx"))
        .unwrap_or(false);

    if !is_docx {
        anyhow::bail!("Expected a .docx file: {}", expanded.display());
    }

    Ok(expanded)
}

/// Reads the raw bytes of `word/document.xml` from a DOCX file.
pub fn read_document_xml(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = File::open(path)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", path.display()))?;

    let mut entry = match archive.by_name("word/document.xml") {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => {
            anyhow::bail!("DOCX is missing word/document.xml: {}", path.display());
        }
        Err(err) => anyhow::bail!("Invalid DOCX file: {}: {err}", path.display()),
    };

    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", path.display()))?;
    Ok(buf)
}

/// Reads an optional member from the DOCX ZIP archive.
pub fn read_optional_zip_member(path: &Path, member: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let mut archive = match ZipArchive::new(file) {
        Ok(a) => a,
        Err(err) => anyhow::bail!("Invalid DOCX file: {}: {err}", path.display()),
    };

    let mut entry = match archive.by_name(member) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => anyhow::bail!("Invalid DOCX file: {}: {err}", path.display()),
    };

    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", path.display()))?;
    Ok(Some(buf))
}

/// Writes a new DOCX file from a source DOCX, replacing the specified parts
/// and preserving all unmodified parts.
pub fn write_package(
    source_path: &Path,
    output_path: &Path,
    overwrite: bool,
    replaced_parts: &HashMap<String, Vec<u8>>,
) -> anyhow::Result<()> {
    if output_path.exists() && !overwrite {
        anyhow::bail!("Output already exists: {}", output_path.display());
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let src_file = File::open(source_path)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", source_path.display()))?;
    let mut src_archive = ZipArchive::new(src_file)
        .map_err(|err| anyhow::anyhow!("Invalid DOCX file: {}: {err}", source_path.display()))?;

    let out_file = File::create(output_path)?;
    let mut out_zip = ZipWriter::new(out_file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Copy unmodified entries
    for i in 0..src_archive.len() {
        let mut entry = src_archive.by_index(i)?;
        let name = entry.name().to_string();

        if replaced_parts.contains_key(&name) {
            continue;
        }

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        out_zip.start_file(name, options)?;
        out_zip.write_all(&data)?;
    }

    // Write replaced parts
    for (name, data) in replaced_parts {
        out_zip.start_file(name, options)?;
        out_zip.write_all(data)?;
    }

    out_zip.finish()?;
    Ok(())
}
