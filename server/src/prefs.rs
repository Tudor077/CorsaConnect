//! The few launcher choices that should still be there tomorrow.
//!
//! One `key=value` line per setting in `%LOCALAPPDATA%\CorsaConnect\launcher.cfg`.
//! Not next to the exe: that folder may be read-only, and a setting that can't
//! be saved is worse than one that doesn't exist.
//!
//! Unknown keys are kept and written back untouched, so an older build reading
//! a newer file doesn't quietly drop settings it didn't recognise.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Prefs {
    values: BTreeMap<String, String>,
}

impl Prefs {
    pub fn load() -> Prefs {
        let mut values = BTreeMap::new();
        if let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    values.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        Prefs { values }
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let body: String = self
            .values
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        let _ = std::fs::write(path, body);
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_string(), value.into());
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.set(key, if value { "true" } else { "false" });
    }
}

fn path() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|d| PathBuf::from(d).join(r"CorsaConnect\launcher.cfg"))
}
