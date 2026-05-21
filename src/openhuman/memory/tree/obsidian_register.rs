//! Best-effort Obsidian vault auto-registration.
//!
//! Obsidian's `obsidian://open?path=...` URI scheme only resolves when
//! the absolute path falls inside a vault that the user has previously
//! added through Obsidian's UI. There is no official URI action to
//! register a new vault — that's an intentional security boundary so
//! arbitrary web pages can't make Obsidian index local folders.
//!
//! Obsidian itself, however, stores its registered-vault list in a plain
//! JSON file at a deterministic per-platform path. Editing that file is
//! how community tools (Obsidian Web Clipper, several mobile sync
//! helpers) ship one-click vault adoption. It's not part of the
//! published API but the schema has stayed stable across the entire
//! 1.x line:
//!
//! ```json
//! {
//!   "vaults": {
//!     "<16-hex-id>": { "path": "/abs/path", "ts": <ms>, "open": <bool> }
//!   },
//!   ...other_keys_we_preserve...
//! }
//! ```
//!
//! This module adds an entry for the memory-tree `content/` root if one
//! isn't already there, leaving every other key in the file untouched.
//! When the file or directory is missing (i.e. Obsidian was never
//! installed or has never launched on this user), we return a structured
//! "not installed" outcome so the UI can fall back to manual
//! instructions instead of crashing.
//!
//! ## Safety
//!
//! - Atomic write via tempfile + rename so a half-written file can't
//!   leave Obsidian unable to start.
//! - Existing entries with a different id but the same path are honoured
//!   (we return `AlreadyPresent` to avoid creating duplicates).
//! - The vault id is a stable hash of the absolute path so re-running
//!   registration produces the same id and the entry is idempotent.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Outcome of an attempted vault registration. Returned to the UI so it
/// can decide whether to dispatch the `obsidian://` URL straight away
/// (`Registered` / `AlreadyPresent`), prompt the user to restart
/// Obsidian, or surface manual installation instructions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RegisterOutcome {
    /// We added a new entry for this path. Existing Obsidian instances
    /// may need a restart to notice; freshly launched ones see it
    /// immediately.
    Registered { config_path: String, vault_id: String },
    /// An entry pointing at the same absolute path was already in the
    /// file. No write needed.
    AlreadyPresent { config_path: String, vault_id: String },
    /// Obsidian's config directory doesn't exist on disk. The user
    /// either hasn't installed Obsidian or has never launched it. UI
    /// should show install / manual-add guidance.
    ObsidianNotInstalled {
        /// The path we would have written to, returned for diagnostics
        /// (the user can run `ls` on it themselves to confirm).
        expected_config_path: String,
    },
}

/// Locate the per-user Obsidian config file. Returns `None` when the
/// platform isn't recognised — we deliberately don't try to read
/// environment variables on unknown OSes because guessing wrong would
/// either silently fail or, worse, write to a random path.
pub fn obsidian_config_path() -> Option<PathBuf> {
    let home = directories::UserDirs::new()?.home_dir().to_path_buf();
    let path = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("obsidian")
            .join("obsidian.json")
    } else if cfg!(target_os = "windows") {
        // %APPDATA% on Windows resolves to <home>\AppData\Roaming
        home.join("AppData")
            .join("Roaming")
            .join("obsidian")
            .join("obsidian.json")
    } else if cfg!(target_os = "linux") {
        home.join(".config").join("obsidian").join("obsidian.json")
    } else {
        return None;
    };
    Some(path)
}

/// Deterministic 16-hex-char vault id derived from the absolute path.
/// Mirrors the shape Obsidian itself uses (`[0-9a-f]{16}`) — collision
/// probability for any real user's vault count is negligible.
fn vault_id_for_path(abs: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Lossless hash over the OS-string bytes; safe for any path the
    // filesystem accepted in the first place.
    abs.as_os_str().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Register `vault_root_abs` as an Obsidian vault by patching
/// `obsidian.json`. See module docs.
pub fn register_vault(vault_root_abs: &Path) -> Result<RegisterOutcome> {
    let config_path = obsidian_config_path().context(
        "unsupported platform — Obsidian auto-registration only handles macOS, Windows, Linux",
    )?;
    log::debug!(
        "[obsidian_register] config_path={} vault={}",
        config_path.display(),
        vault_root_abs.display()
    );

    if !config_path.parent().map(|p| p.exists()).unwrap_or(false) {
        log::info!(
            "[obsidian_register] config directory missing — Obsidian not installed or never launched (dir={:?})",
            config_path.parent()
        );
        return Ok(RegisterOutcome::ObsidianNotInstalled {
            expected_config_path: config_path.display().to_string(),
        });
    }

    // Load existing config. A missing file is fine — Obsidian creates it
    // on first launch with the same shape; we can pre-create it.
    let mut root: Map<String, Value> = match std::fs::read(&config_path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .with_context(|| format!("parse obsidian.json at {}", config_path.display()))?
            .as_object()
            .cloned()
            .unwrap_or_default(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(err) => {
            return Err(anyhow::anyhow!(err)).with_context(|| {
                format!("read obsidian.json at {}", config_path.display())
            });
        }
    };

    // `vaults` may be absent (fresh install before any vault was added);
    // treat as empty.
    let vaults_value = root
        .entry("vaults".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(vaults) = vaults_value.as_object_mut() else {
        anyhow::bail!(
            "obsidian.json `vaults` is not a JSON object — refusing to overwrite \
             (path={})",
            config_path.display()
        );
    };

    // Idempotency: if any existing entry already points at the same
    // absolute path, return its id rather than minting a duplicate.
    let target_abs = vault_root_abs.display().to_string();
    for (existing_id, entry) in vaults.iter() {
        if let Some(p) = entry.get("path").and_then(|v| v.as_str()) {
            if p == target_abs {
                log::debug!(
                    "[obsidian_register] vault already registered id={} path={}",
                    existing_id,
                    target_abs
                );
                return Ok(RegisterOutcome::AlreadyPresent {
                    config_path: config_path.display().to_string(),
                    vault_id: existing_id.clone(),
                });
            }
        }
    }

    // New entry. Use a deterministic id so re-running registration after
    // a manual purge produces the same key — keeps Obsidian's internal
    // per-vault settings (graph layout, hotkeys) attached.
    let vault_id = vault_id_for_path(vault_root_abs);
    let entry = serde_json::json!({
        "path": target_abs,
        "ts": now_unix_ms(),
        // `open: false` so we don't fight whatever vault the user has
        // currently focused; the `obsidian://open?path=` URI that fires
        // right after this write will set focus to the new vault for
        // exactly this session.
        "open": false,
    });
    vaults.insert(vault_id.clone(), entry);

    let body = serde_json::to_vec_pretty(&Value::Object(root))
        .context("serialize patched obsidian.json")?;
    write_atomically(&config_path, &body)?;

    log::info!(
        "[obsidian_register] registered vault id={} path={} config={}",
        vault_id,
        target_abs,
        config_path.display()
    );
    Ok(RegisterOutcome::Registered {
        config_path: config_path.display().to_string(),
        vault_id,
    })
}

/// Atomic write: stage as `<dest>.tmp` in the same directory (so rename
/// stays on the same filesystem) and then `rename` over the destination.
/// Failure midway leaves the original file intact.
fn write_atomically(dest: &Path, body: &[u8]) -> Result<()> {
    let parent = dest
        .parent()
        .context("destination has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| {
        format!("create parent directory for {}", dest.display())
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("obsidian")
    ));
    std::fs::write(&tmp, body)
        .with_context(|| format!("write temp file {}", tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| {
        format!("rename {} -> {}", tmp.display(), dest.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn config_path_resolves_on_supported_platforms() {
        // We can't easily mock target_os, but the resolver should at
        // least return Some(...) on any of the supported platforms the
        // build matrix runs on.
        let p = obsidian_config_path();
        if cfg!(any(target_os = "macos", target_os = "linux", target_os = "windows")) {
            assert!(p.is_some(), "expected a config path on this platform");
            let p = p.unwrap();
            assert!(p.ends_with("obsidian.json"), "got {}", p.display());
        }
    }

    #[test]
    fn vault_id_is_stable() {
        let a = vault_id_for_path(Path::new("/Users/foo/vault"));
        let b = vault_id_for_path(Path::new("/Users/foo/vault"));
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn vault_id_varies_with_path() {
        let a = vault_id_for_path(Path::new("/Users/foo/vault"));
        let b = vault_id_for_path(Path::new("/Users/foo/other"));
        assert_ne!(a, b);
    }

    /// Build a fake obsidian.json fixture in `dir` and stub the resolver
    /// by writing directly to the path the helper computed for us. The
    /// shape mirrors Obsidian's real file so we exercise the merge path.
    fn write_fixture(dir: &Path, body: &str) -> PathBuf {
        let target = dir.join("obsidian.json");
        std::fs::write(&target, body).unwrap();
        target
    }

    /// Direct-path variant of `register_vault` so tests don't depend on
    /// the real user's `~/Library/Application Support/obsidian/...`.
    fn register_vault_at(
        config_path: &Path,
        vault_root_abs: &Path,
    ) -> Result<RegisterOutcome> {
        // Mirror the production logic but skip the platform resolver so
        // tests are hermetic. Kept in sync manually — small enough that
        // drift is easy to spot in review.
        if !config_path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Ok(RegisterOutcome::ObsidianNotInstalled {
                expected_config_path: config_path.display().to_string(),
            });
        }
        let mut root: Map<String, Value> = match std::fs::read(config_path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .unwrap()
                .as_object()
                .cloned()
                .unwrap_or_default(),
            Err(_) => Map::new(),
        };
        let vaults_value = root
            .entry("vaults".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let vaults = vaults_value.as_object_mut().unwrap();
        let target_abs = vault_root_abs.display().to_string();
        for (existing_id, entry) in vaults.iter() {
            if let Some(p) = entry.get("path").and_then(|v| v.as_str()) {
                if p == target_abs {
                    return Ok(RegisterOutcome::AlreadyPresent {
                        config_path: config_path.display().to_string(),
                        vault_id: existing_id.clone(),
                    });
                }
            }
        }
        let vault_id = vault_id_for_path(vault_root_abs);
        vaults.insert(
            vault_id.clone(),
            serde_json::json!({
                "path": target_abs,
                "ts": now_unix_ms(),
                "open": false,
            }),
        );
        let body = serde_json::to_vec_pretty(&Value::Object(root)).unwrap();
        write_atomically(config_path, &body)?;
        Ok(RegisterOutcome::Registered {
            config_path: config_path.display().to_string(),
            vault_id,
        })
    }

    #[test]
    fn registered_when_file_missing_but_dir_exists() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("obsidian.json");
        let vault = tmp.path().join("my_vault");
        std::fs::create_dir_all(&vault).unwrap();
        let out = register_vault_at(&cfg, &vault).unwrap();
        match out {
            RegisterOutcome::Registered { vault_id, .. } => {
                assert_eq!(vault_id, vault_id_for_path(&vault));
                // File now exists and contains our vault.
                let body = std::fs::read_to_string(&cfg).unwrap();
                let parsed: Value = serde_json::from_str(&body).unwrap();
                let stored = parsed["vaults"][&vault_id]["path"]
                    .as_str()
                    .unwrap();
                assert_eq!(stored, vault.display().to_string());
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn already_present_when_vault_path_matches_existing_entry() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let fixture = format!(
            r#"{{"vaults":{{"abc1234567890def":{{"path":"{}","ts":1,"open":true}}}}}}"#,
            vault.display()
        );
        let cfg = write_fixture(tmp.path(), &fixture);
        let out = register_vault_at(&cfg, &vault).unwrap();
        match out {
            RegisterOutcome::AlreadyPresent { vault_id, .. } => {
                assert_eq!(vault_id, "abc1234567890def");
            }
            other => panic!("expected AlreadyPresent, got {other:?}"),
        }
        // File contents should be byte-identical (no rewrite).
        let body = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(body, fixture);
    }

    #[test]
    fn preserves_existing_vaults_and_unrelated_keys() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("new_vault");
        std::fs::create_dir_all(&vault).unwrap();
        let fixture = r#"{
            "vaults": {
                "existing_id_aaaa": {"path":"/Users/u/other","ts":1,"open":true}
            },
            "frontmatter": {"some":"setting"}
        }"#;
        let cfg = write_fixture(tmp.path(), fixture);
        let out = register_vault_at(&cfg, &vault).unwrap();
        assert!(matches!(out, RegisterOutcome::Registered { .. }));
        let body = std::fs::read_to_string(&cfg).unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            parsed["vaults"]["existing_id_aaaa"]["path"],
            "/Users/u/other"
        );
        assert_eq!(parsed["frontmatter"]["some"], "setting");
        // The new entry exists at the computed id.
        let new_id = vault_id_for_path(&vault);
        assert!(parsed["vaults"][&new_id].is_object());
    }

    #[test]
    fn not_installed_when_parent_directory_missing() {
        let tmp = TempDir::new().unwrap();
        // Parent dir we point at does NOT exist.
        let cfg = tmp.path().join("nonexistent_dir").join("obsidian.json");
        let vault = tmp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let out = register_vault_at(&cfg, &vault).unwrap();
        assert!(matches!(out, RegisterOutcome::ObsidianNotInstalled { .. }));
    }
}
