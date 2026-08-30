//! Port of `utils/zotero_paths.py`'s discovery chain (only the read paths
//! needed by the vertical slice — plugin install/uninstall management is
//! not ported yet). Every fallback, in every function, mirrors the Python
//! source order exactly; see the file's own doc comments for the exact
//! Python line references this was checked against.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;

const DATA_DIR_PREF: &str = "extensions.zotero.dataDir";
const USE_DATA_DIR_PREF: &str = "extensions.zotero.useDataDir";
const LOCAL_API_PREF: &str = "extensions.zotero.httpServer.localAPI.enabled";
const HTTP_PORT_PREF: &str = "extensions.zotero.httpServer.port";

#[derive(Debug, Clone, Serialize)]
pub struct ZoteroEnvironment {
    pub executable: Option<PathBuf>,
    pub executable_exists: bool,
    pub install_dir: Option<PathBuf>,
    pub version: String,
    pub profile_root: PathBuf,
    pub profile_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub data_dir_exists: bool,
    pub sqlite_path: PathBuf,
    pub sqlite_exists: bool,
    pub styles_dir: PathBuf,
    pub styles_exists: bool,
    pub storage_dir: PathBuf,
    pub storage_exists: bool,
    pub translators_dir: PathBuf,
    pub translators_exists: bool,
    pub port: u16,
    pub local_api_enabled_configured: bool,
}

fn home_dir() -> PathBuf {
    // Matches Python's Path.home(): no ZOTERO_DATA_DIR-style override here,
    // this is the raw OS home directory.
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn env_trimmed(env_vars: &HashMap<String, String>, key: &str) -> Option<String> {
    env_vars
        .get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `candidate_profile_roots()` (`zotero_paths.py:49-67`).
pub fn candidate_profile_roots(env_vars: &HashMap<String, String>) -> Vec<PathBuf> {
    let home = home_dir();
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    };
    if let Some(appdata) = env_vars.get("APPDATA").filter(|v| !v.is_empty()) {
        add(PathBuf::from(appdata).join("Zotero").join("Zotero"));
    }
    add(home
        .join("AppData")
        .join("Roaming")
        .join("Zotero")
        .join("Zotero"));
    add(home
        .join("Library")
        .join("Application Support")
        .join("Zotero"));
    add(home.join(".zotero").join("zotero"));
    candidates
}

/// `find_profile_root()` (`zotero_paths.py:70-89`).
pub fn find_profile_root(
    explicit_profile_dir: Option<&str>,
    env_vars: &HashMap<String, String>,
) -> PathBuf {
    if let Some(explicit) = explicit_profile_dir {
        let explicit = expand_user_path(explicit);
        if explicit
            .file_name()
            .map(|n| n == "profiles.ini")
            .unwrap_or(false)
        {
            return explicit.parent().map(Path::to_path_buf).unwrap_or(explicit);
        }
        if explicit.join("profiles.ini").exists() {
            return explicit;
        }
        if let Some(parent) = explicit.parent() {
            if parent.join("profiles.ini").exists() {
                return parent.to_path_buf();
            }
        }
        return explicit;
    }

    if let Some(env_profile) = env_trimmed(env_vars, "ZOTERO_PROFILE_DIR") {
        return find_profile_root(Some(&env_profile), env_vars);
    }

    let candidates = candidate_profile_roots(env_vars);
    for candidate in &candidates {
        if candidate.join("profiles.ini").exists() {
            return candidate.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_user_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if path == "~" {
        return home_dir();
    }
    PathBuf::from(path)
}

/// Minimal INI parser matching Python `configparser` defaults closely
/// enough for `profiles.ini`: section names kept as-written, option
/// (key) names lowercased on both read and lookup, `#`/`;` line comments.
struct IniFile {
    sections: Vec<(String, HashMap<String, String>)>,
}

impl IniFile {
    fn parse(text: &str) -> Self {
        let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
        let mut current: Option<usize> = None;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = line[1..line.len() - 1].to_string();
                sections.push((name, HashMap::new()));
                current = Some(sections.len() - 1);
                continue;
            }
            if let Some(idx) = current {
                if let Some((key, value)) = line.split_once('=') {
                    sections[idx]
                        .1
                        .insert(key.trim().to_lowercase(), value.trim().to_string());
                }
            }
        }
        IniFile { sections }
    }

    fn section_names(&self) -> Vec<&str> {
        self.sections
            .iter()
            .map(|(name, _)| name.as_str())
            .collect()
    }

    fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(name, _)| name == section)
            .and_then(|(_, kv)| kv.get(&key.to_lowercase()))
            .map(String::as_str)
    }
}

/// `find_active_profile()` + `_profile_path_from_section()` (`zotero_paths.py:100-119`).
pub fn find_active_profile(profile_root: &Path) -> Option<PathBuf> {
    let ini_path = profile_root.join("profiles.ini");
    let text = std::fs::read_to_string(&ini_path).unwrap_or_default();
    let ini = IniFile::parse(&text);
    let ordered_sections: Vec<&str> = ini
        .section_names()
        .into_iter()
        .filter(|s| s.to_lowercase().starts_with("profile"))
        .collect();

    let path_from_section = |section: &str| -> Option<PathBuf> {
        let path_value = ini.get(section, "Path").unwrap_or("").trim();
        if path_value.is_empty() {
            return None;
        }
        let is_relative = ini.get(section, "IsRelative").unwrap_or("1").trim() == "1";
        if is_relative {
            Some(normalize_resolve(&profile_root.join(path_value)))
        } else {
            Some(expand_user_path(path_value))
        }
    };

    for section in &ordered_sections {
        let is_default = ini.get(section, "Default").unwrap_or("0").trim() == "1";
        if is_default {
            if let Some(p) = path_from_section(section) {
                return Some(p);
            }
        }
    }
    for section in &ordered_sections {
        if let Some(p) = path_from_section(section) {
            return Some(p);
        }
    }
    None
}

/// Approximates Python's `Path.resolve()` in its default non-strict mode:
/// normalizes `.`/`..` and resolves symlinks where the path exists, but
/// unlike `std::fs::canonicalize` does not require the full path to exist.
///
/// `pub(crate)`: also used by `db::resolve_attachment_real_path`, which
/// ports `resolve_attachment_real_path()`'s two `.resolve()` call sites
/// (`zotero_sqlite.py:552-574`) — kept as one implementation rather than
/// duplicated, per the project's DRY principle.
pub(crate) fn normalize_resolve(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn read_pref_file(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn decode_pref_string(raw: &str) -> String {
    raw.replace("\\\\", "\\").replace("\\\"", "\"")
}

/// `read_pref()` (`zotero_paths.py:137-153`): `user.js` takes priority
/// over `prefs.js`.
pub fn read_pref(profile_dir: Option<&Path>, pref_name: &str) -> Option<String> {
    let profile_dir = profile_dir?;
    let pattern = Regex::new(&format!(
        r#"user_pref\("{}",\s*(.+?)\);"#,
        regex::escape(pref_name)
    ))
    .ok()?;
    for filename in ["user.js", "prefs.js"] {
        let text = read_pref_file(&profile_dir.join(filename));
        for line in text.lines() {
            let Some(captures) = pattern.captures(line) else {
                continue;
            };
            let raw = captures.get(1).unwrap().as_str().trim();
            if raw == "true" || raw == "false" {
                return Some(raw.to_string());
            }
            if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
                return Some(decode_pref_string(&raw[1..raw.len() - 1]));
            }
            return Some(raw.to_string());
        }
    }
    None
}

/// `find_data_dir()` (`zotero_paths.py:156-173`).
pub fn find_data_dir(
    profile_dir: Option<&Path>,
    explicit_data_dir: Option<&str>,
    env_vars: &HashMap<String, String>,
) -> PathBuf {
    if let Some(explicit) = explicit_data_dir {
        return expand_user_path(explicit);
    }
    if let Some(env_dir) = env_trimmed(env_vars, "ZOTERO_DATA_DIR") {
        return expand_user_path(&env_dir);
    }
    if let Some(profile_dir) = profile_dir {
        let use_data_dir = read_pref(Some(profile_dir), USE_DATA_DIR_PREF);
        let pref_data_dir = read_pref(Some(profile_dir), DATA_DIR_PREF);
        if use_data_dir.as_deref() == Some("true") {
            if let Some(pref_data_dir) = pref_data_dir.filter(|v| !v.is_empty()) {
                let candidate = expand_user_path(&pref_data_dir);
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }
    home_dir().join("Zotero")
}

/// `find_executable()` (`zotero_paths.py:176-200`).
pub fn find_executable(
    explicit_executable: Option<&str>,
    env_vars: &HashMap<String, String>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit_executable {
        return Some(expand_user_path(explicit));
    }
    if let Some(env_exec) = env_trimmed(env_vars, "ZOTERO_EXECUTABLE") {
        return Some(expand_user_path(&env_exec));
    }
    for name in ["zotero", "zotero.exe"] {
        if let Ok(path) = which(name) {
            return Some(path);
        }
    }
    let candidates = [
        PathBuf::from(r"C:\Program Files\Zotero\zotero.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Zotero\zotero.exe"),
        PathBuf::from("/Applications/Zotero.app/Contents/MacOS/zotero"),
        PathBuf::from("/usr/lib/zotero/zotero"),
        PathBuf::from("/usr/local/bin/zotero"),
    ];
    candidates.into_iter().find(|c| c.exists())
}

/// Minimal `shutil.which()` equivalent: search `PATH` for an executable
/// file with the given name.
fn which(name: &str) -> std::io::Result<PathBuf> {
    let path_var = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "not found",
    ))
}

/// `get_version()` (`zotero_paths.py:209-224`).
pub fn get_version(install_dir: Option<&Path>) -> String {
    let Some(install_dir) = install_dir else {
        return "unknown".to_string();
    };
    let candidates = [
        install_dir.join("app").join("application.ini"),
        install_dir.join("application.ini"),
        install_dir
            .parent()
            .map(|p| p.join("Resources").join("app").join("application.ini"))
            .unwrap_or_else(|| install_dir.join("Resources/app/application.ini")),
    ];
    let version_re = Regex::new(r"(?m)^Version=(.+)$").unwrap();
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        let text = read_pref_file(&candidate);
        if let Some(captures) = version_re.captures(&text) {
            return captures.get(1).unwrap().as_str().trim().to_string();
        }
    }
    "unknown".to_string()
}

/// `get_http_port()` (`zotero_paths.py:227-241`).
pub fn get_http_port(profile_dir: Option<&Path>, env_vars: &HashMap<String, String>) -> u16 {
    if let Some(env_port) = env_trimmed(env_vars, "ZOTERO_HTTP_PORT") {
        if let Ok(port) = env_port.parse::<u16>() {
            return port;
        }
    }
    if let Some(pref_port) = read_pref(profile_dir, HTTP_PORT_PREF) {
        if let Ok(port) = pref_port.parse::<u16>() {
            return port;
        }
    }
    23119
}

/// `is_local_api_enabled()` (`zotero_paths.py:244-245`).
pub fn is_local_api_enabled(profile_dir: Option<&Path>) -> bool {
    read_pref(profile_dir, LOCAL_API_PREF).as_deref() == Some("true")
}

/// `build_environment()` (`zotero_paths.py:248-289`).
pub fn build_environment(
    explicit_data_dir: Option<&str>,
    explicit_profile_dir: Option<&str>,
    explicit_executable: Option<&str>,
    env_vars: &HashMap<String, String>,
) -> ZoteroEnvironment {
    let profile_root = find_profile_root(explicit_profile_dir, env_vars);
    let env_profile_dir = env_trimmed(env_vars, "ZOTERO_PROFILE_DIR");
    let explicit_or_env_profile = explicit_profile_dir.map(str::to_string).or(env_profile_dir);
    let profile_dir = match &explicit_or_env_profile {
        Some(p) if PathBuf::from(p).join("prefs.js").exists() => Some(expand_user_path(p)),
        _ => find_active_profile(&profile_root),
    };
    let executable = find_executable(explicit_executable, env_vars);
    let install_dir = executable
        .as_deref()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let data_dir = find_data_dir(profile_dir.as_deref(), explicit_data_dir, env_vars);
    let sqlite_path = data_dir.join("zotero.sqlite");
    let styles_dir = data_dir.join("styles");
    let storage_dir = data_dir.join("storage");
    let translators_dir = data_dir.join("translators");
    let executable_exists = executable.as_deref().map(Path::exists).unwrap_or(false);

    ZoteroEnvironment {
        executable_exists,
        version: get_version(install_dir.as_deref()),
        install_dir,
        data_dir_exists: data_dir.exists(),
        sqlite_exists: sqlite_path.exists(),
        styles_exists: styles_dir.exists(),
        storage_exists: storage_dir.exists(),
        translators_exists: translators_dir.exists(),
        port: get_http_port(profile_dir.as_deref(), env_vars),
        local_api_enabled_configured: is_local_api_enabled(profile_dir.as_deref()),
        executable,
        profile_root,
        profile_dir,
        data_dir,
        sqlite_path,
        styles_dir,
        storage_dir,
        translators_dir,
    }
}

pub fn current_env_map() -> HashMap<String, String> {
    env::vars().collect()
}

/// `plugin_xpi_path()` (`zotero_paths.py:300-304`).
pub fn plugin_xpi_path(profile_dir: Option<&Path>) -> Option<PathBuf> {
    profile_dir.map(|p| p.join("extensions").join(crate::plugin::XPI_FILENAME))
}

/// `plugin_installed()` (`zotero_paths.py:307-312`).
pub fn plugin_installed(profile_dir: Option<&Path>) -> bool {
    let Some(profile_dir) = profile_dir else {
        return false;
    };
    let our_xpi = profile_dir
        .join("extensions")
        .join(crate::plugin::XPI_FILENAME);
    if our_xpi.is_file() {
        return true;
    }
    let upstream_xpi = profile_dir
        .join("extensions")
        .join(crate::plugin::UPSTREAM_XPI_FILENAME);
    upstream_xpi.is_file()
}

/// `bundled_plugin_version()` (`zotero_paths.py:320-330`).
pub fn bundled_plugin_version() -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(crate::plugin::MANIFEST_JSON).ok()?;
    payload
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// `installed_plugin_version()` (`zotero_paths.py:333-344`).
pub fn installed_plugin_version(profile_dir: Option<&Path>) -> Option<String> {
    let profile_dir = profile_dir?;
    let our_xpi = profile_dir
        .join("extensions")
        .join(crate::plugin::XPI_FILENAME);
    let xpi_path = if our_xpi.is_file() {
        our_xpi
    } else {
        let upstream_xpi = profile_dir
            .join("extensions")
            .join(crate::plugin::UPSTREAM_XPI_FILENAME);
        if upstream_xpi.is_file() {
            upstream_xpi
        } else {
            return None;
        }
    };
    let file = std::fs::File::open(&xpi_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut manifest_file = archive.by_name("manifest.json").ok()?;
    let mut text = String::new();
    use std::io::Read;
    manifest_file.read_to_string(&mut text).ok()?;
    let payload: serde_json::Value = serde_json::from_str(&text).ok()?;
    payload
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// `plugin_update_available()` (`zotero_paths.py:347-351`).
pub fn plugin_update_available(profile_dir: Option<&Path>) -> bool {
    let installed = installed_plugin_version(profile_dir);
    let bundled = bundled_plugin_version();
    installed.is_some() && bundled.is_some() && installed != bundled
}
