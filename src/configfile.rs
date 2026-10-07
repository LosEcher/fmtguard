//! Configuration files (P1f-4).
//!
//! Sources, lowest precedence first: built-in defaults → user file
//! (`~/.config/fmtguard/config.toml`) → repo file (`<repo>/.fmtguard.toml`) →
//! command-line flags. Only *policy* keys can come from a file; per-invocation
//! switches (`--apply`, `--sandbox`, `--emit`, `--changeset`, `--no-log`,
//! `--require-version`) deliberately cannot — a config file that silently
//! enables writing is exactly the class of surprise this tool exists to
//! prevent.
//!
//! The parser covers the TOML subset the config needs (comments, bare keys,
//! `[section]`, strings, integers, floats, booleans, arrays of strings) and
//! rejects everything else loudly. A config file is small and hand-written;
//! "unsupported syntax at line N" beats a wrong value applied silently.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<String>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[String]> {
        match self {
            Value::Array(v) => Some(v),
            _ => None,
        }
    }
}

/// Keys a config file may set. Anything else is an error (typo protection).
pub const KNOWN_KEYS: &[&str] = &[
    "rustfmt",
    "engine",
    "engine_timeout_secs",
    "hunk_context",
    "diff_timeout_secs",
    "include_untracked",
    "verify_fmt_check",
    "log",
    "base",
    "exclude",
    "budget.max_added_lines",
    "budget.max_files",
    "budget.max_ratio",
];

#[derive(Debug, Clone, Default)]
pub struct FileConfig {
    pub path: PathBuf,
    pub values: BTreeMap<String, Value>,
}

fn strip_comment(line: &str) -> &str {
    // A '#' inside a quoted string is content, not a comment.
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_scalar(raw: &str, line_no: usize) -> Result<Value, String> {
    let t = raw.trim();
    if t.starts_with('"') {
        if !t.ends_with('"') || t.len() < 2 {
            return Err(format!("line {line_no}: unterminated string"));
        }
        return Ok(Value::Str(t[1..t.len() - 1].to_string()));
    }
    if t.starts_with('[') {
        if !t.ends_with(']') {
            return Err(format!("line {line_no}: unterminated array"));
        }
        let inner = &t[1..t.len() - 1];
        let mut items = Vec::new();
        for part in inner.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if !p.starts_with('"') || !p.ends_with('"') || p.len() < 2 {
                return Err(format!(
                    "line {line_no}: only arrays of quoted strings are supported"
                ));
            }
            items.push(p[1..p.len() - 1].to_string());
        }
        return Ok(Value::Array(items));
    }
    match t {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Ok(i) = t.parse::<i64>() {
        return Ok(Value::Int(i));
    }
    if let Ok(f) = t.parse::<f64>() {
        return Ok(Value::Float(f));
    }
    Err(format!(
        "line {line_no}: unsupported value `{t}` (strings must be quoted)"
    ))
}

/// Parse the supported TOML subset into dotted keys (`budget.max_ratio`).
pub fn parse(text: &str) -> Result<BTreeMap<String, Value>, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("line {line_no}: malformed section header"));
            }
            let name = line[1..line.len() - 1].trim();
            if name.is_empty()
                || name
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
            {
                return Err(format!(
                    "line {line_no}: unsupported section header `{line}` (flat `[section]` names \
                     only; arrays of tables and nested tables are not supported)"
                ));
            }
            section = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {line_no}: expected `key = value` (nested tables and multi-line values are \
                 not supported)"
            ));
        };
        let key = key.trim();
        if key.is_empty() || key.contains(['[', ']', '{', '}']) {
            return Err(format!("line {line_no}: unsupported key `{key}`"));
        }
        let dotted = if section.is_empty() {
            key.to_string()
        } else {
            format!("{section}.{key}")
        };
        if !KNOWN_KEYS.contains(&dotted.as_str()) {
            return Err(format!(
                "line {line_no}: unknown config key `{dotted}` (known: {})",
                KNOWN_KEYS.join(", ")
            ));
        }
        out.insert(dotted, parse_scalar(value, line_no)?);
    }
    Ok(out)
}

pub fn load(path: &Path) -> Result<Option<FileConfig>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let values = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(FileConfig {
        path: path.to_path_buf(),
        values,
    }))
}

/// `~/.config/fmtguard/config.toml` (honours `$XDG_CONFIG_HOME`).
pub fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("fmtguard/config.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config/fmtguard/config.toml"))
}

pub fn repo_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".fmtguard.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_supported_subset() {
        let text = r#"
# a comment
rustfmt = "/opt/rustfmt"
engine_timeout_secs = 60
include_untracked = true
verify_fmt_check = "strict"
base = "main"   # trailing comment
exclude = ["src/legacy/**", "examples/**"]

[budget]
max_added_lines = 500
max_ratio = 1.5
"#;
        let v = parse(text).unwrap();
        assert_eq!(v["rustfmt"], Value::Str("/opt/rustfmt".into()));
        assert_eq!(v["engine_timeout_secs"], Value::Int(60));
        assert_eq!(v["include_untracked"], Value::Bool(true));
        assert_eq!(v["verify_fmt_check"], Value::Str("strict".into()));
        assert_eq!(v["base"], Value::Str("main".into()));
        assert_eq!(
            v["exclude"],
            Value::Array(vec!["src/legacy/**".into(), "examples/**".into()])
        );
        assert_eq!(v["budget.max_added_lines"], Value::Int(500));
        assert_eq!(v["budget.max_ratio"], Value::Float(1.5));
    }

    #[test]
    fn rejects_unknown_keys_and_exotic_syntax() {
        assert!(parse("nope = 1\n")
            .unwrap_err()
            .contains("unknown config key"));
        assert!(parse("rustfmt = rustfmt\n").unwrap_err().contains("quoted"));
        assert!(parse("rustfmt =\n")
            .unwrap_err()
            .contains("unsupported value"));
        assert!(parse("[[table]]\n")
            .unwrap_err()
            .contains("unsupported section header"));
        // a '#' inside a string is content, not a comment
        let v = parse("rustfmt = \"a#b\"\n").unwrap();
        assert_eq!(v["rustfmt"], Value::Str("a#b".into()));
    }
}
