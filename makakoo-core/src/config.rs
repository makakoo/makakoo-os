//! Persona config loader.
//!
//! Reads `{makakoo_home}/config/persona.json`. Fresh installs get a
//! neutral "Makakoo" default; the live installation of a returning
//! user (whose persona.json has `name: "Harvey"` or anything else)
//! overrides the default cleanly. New users name their assistant
//! interactively via `makakoo setup`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::platform::makakoo_home;

/// Canonical on-disk location of `persona.json` for a given home dir.
pub fn persona_path_for(home: &Path) -> PathBuf {
    home.join("config").join("persona.json")
}

/// Canonical on-disk location for the current process's MAKAKOO_HOME.
pub fn persona_path() -> PathBuf {
    persona_path_for(&makakoo_home())
}

/// Default persona name — neutral brand-level fallback used when a fresh
/// install has no persona.json yet. Interactive `makakoo setup` replaces
/// this with whatever the user picks.
fn default_persona_name() -> String {
    "Makakoo".to_string()
}

/// Five suggested names offered by `makakoo setup`. Option 6 in the
/// wizard is always "type your own". These are kept here (rather than
/// in the CLI crate) so non-interactive tools can show the same list.
pub const SUGGESTED_NAMES: &[&str] = &["Makakoo", "Bongo", "Kai", "Nova", "Sage"];

/// Parse a wizard choice string into a resolved name.
///
/// Accepts:
///   - "1".."5" → the matching `SUGGESTED_NAMES[n-1]`
///   - "6" with a non-empty `custom` → trimmed custom name
///   - anything else → `None` (caller should reprompt)
///
/// Pure function — no I/O — so the wizard flow is unit-testable.
pub fn resolve_name_choice(choice: &str, custom: Option<&str>) -> Option<String> {
    let trimmed = choice.trim();
    match trimmed.parse::<usize>() {
        Ok(n) if (1..=SUGGESTED_NAMES.len()).contains(&n) => {
            Some(SUGGESTED_NAMES[n - 1].to_string())
        }
        Ok(n) if n == SUGGESTED_NAMES.len() + 1 => custom
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Default pronoun ("they"). T18 — the user's live `config/persona.json`
/// ships with `pronouns: "he/him"` (plural, with slash) and no `pronoun`
/// singular field. The loader accepts both, so either the `pronoun` or
/// `pronouns` key is honored; if both are missing we fall back to `they`
/// as the safest neutral default.
fn default_pronoun() -> String {
    "they".to_string()
}

/// Default voice ("caveman") — matches the user's original install.
fn default_voice_default() -> String {
    "caveman".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonaConfig {
    #[serde(default = "default_persona_name")]
    pub name: String,
    /// Pronoun. Accepts either a Rust-era `pronoun` field OR the legacy
    /// Python-era `pronouns` alias (e.g. `"he/him"`). Defaults to `"they"`.
    #[serde(default = "default_pronoun", alias = "pronouns")]
    pub pronoun: String,
    #[serde(default = "default_voice_default")]
    pub voice_default: String,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            name: default_persona_name(),
            pronoun: default_pronoun(),
            voice_default: default_voice_default(),
        }
    }
}

impl PersonaConfig {
    /// Serialize this config as pretty JSON and write it atomically to
    /// `path`. Parent directories are created on demand. Used by
    /// `makakoo setup` and any future config-editing flow.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        // Write to a sibling tmp path then rename — atomic on POSIX.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, body)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Load the persona config. Returns defaults if the file is missing or
/// cannot be parsed as JSON. T18 — serde defaults make any subset of
/// fields load cleanly, so a partial persona.json (only `name` + `user`,
/// the canonical production shape) deserialises silently without a warning.
pub fn load_persona() -> Result<PersonaConfig> {
    let path = persona_path();
    if !path.exists() {
        return Ok(PersonaConfig::default());
    }
    let raw = fs::read_to_string(&path)?;
    match serde_json::from_str::<PersonaConfig>(&raw) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            // Only reached on hard JSON parse errors (malformed syntax,
            // wrong type for `name`, etc.). Missing fields no longer land
            // here — they hit the `#[serde(default = ...)]` path silently.
            tracing::warn!(
                "persona.json at {} is malformed: {} — falling back to defaults",
                path.display(),
                e
            );
            Ok(PersonaConfig::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_neutral_brand_name() {
        // Fresh installs that never ran `makakoo setup` land on the
        // brand-level default. Returning users with a populated
        // persona.json override this at load time.
        let p = PersonaConfig::default();
        assert_eq!(p.name, "Makakoo");
        assert_eq!(p.pronoun, "they");
        assert_eq!(p.voice_default, "caveman");
    }

    #[test]
    fn suggested_names_has_five_and_starts_with_brand() {
        assert_eq!(SUGGESTED_NAMES.len(), 5);
        assert_eq!(SUGGESTED_NAMES[0], "Makakoo");
    }

    #[test]
    fn resolve_name_choice_covers_suggestions_custom_and_garbage() {
        assert_eq!(resolve_name_choice("1", None).as_deref(), Some("Makakoo"));
        assert_eq!(resolve_name_choice("5", None).as_deref(), Some("Sage"));
        assert_eq!(
            resolve_name_choice("6", Some("Jarvis")).as_deref(),
            Some("Jarvis")
        );
        // Whitespace trimming on both the index and the custom name.
        assert_eq!(
            resolve_name_choice("  6 ", Some("  Atlas  ")).as_deref(),
            Some("Atlas")
        );
        // Out-of-range index → None (caller reprompts).
        assert_eq!(resolve_name_choice("0", None), None);
        assert_eq!(resolve_name_choice("7", None), None);
        // Custom selected but empty → None.
        assert_eq!(resolve_name_choice("6", Some("   ")), None);
        assert_eq!(resolve_name_choice("6", None), None);
        // Non-numeric → None.
        assert_eq!(resolve_name_choice("banana", None), None);
    }

    #[test]
    fn save_to_writes_round_trippable_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("persona.json");
        let p = PersonaConfig {
            name: "Atlas".into(),
            pronoun: "they".into(),
            voice_default: "caveman".into(),
        };
        p.save_to(&path).unwrap();
        assert!(path.exists(), "save_to should create the file");

        let raw = std::fs::read_to_string(&path).unwrap();
        let back: PersonaConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, p);

        // No leftover tmp file.
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn roundtrip_serde() {
        let p = PersonaConfig {
            name: "Olibia".into(),
            pronoun: "she".into(),
            voice_default: "friendly".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: PersonaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn load_persona_returns_default_when_missing() {
        // Redirect MAKAKOO_HOME to an empty tempdir so we know the file
        // does not exist.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let cfg = load_persona().unwrap();
        assert_eq!(cfg, PersonaConfig::default());
        std::env::remove_var("MAKAKOO_HOME");
    }

    /// T18 — the user's real persona.json has `{name, user, pronouns,
    /// home}` and lacks `pronoun` + `voice_default`. Must load without
    /// warning and populate missing fields via serde defaults, with the
    /// plural-legacy `pronouns` alias feeding the `pronoun` field.
    #[test]
    fn load_persona_accepts_legacy_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("persona.json"),
            r#"{"version":1,"name":"Harvey","user":"tester","pronouns":"they/them","home":"/tmp/makakoo-test"}"#,
        )
        .unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let cfg = load_persona().unwrap();
        std::env::remove_var("MAKAKOO_HOME");
        assert_eq!(cfg.name, "Harvey");
        assert_eq!(cfg.pronoun, "they/them"); // via alias
        assert_eq!(cfg.voice_default, "caveman"); // via serde default
    }

    /// T18 — when neither `pronoun` nor `pronouns` is present, the serde
    /// default kicks in and returns the neutral `they`. No warning.
    #[test]
    fn load_persona_pronoun_default_is_they_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("persona.json"),
            r#"{"name":"Makakoo"}"#,
        )
        .unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let cfg = load_persona().unwrap();
        std::env::remove_var("MAKAKOO_HOME");
        assert_eq!(cfg.name, "Makakoo");
        assert_eq!(cfg.pronoun, "they");
        assert_eq!(cfg.voice_default, "caveman");
    }

    #[test]
    fn load_persona_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("persona.json"),
            r#"{"name":"TestHero","pronoun":"they","voice_default":"formal"}"#,
        )
        .unwrap();
        std::env::set_var("MAKAKOO_HOME", dir.path());
        let cfg = load_persona().unwrap();
        std::env::remove_var("MAKAKOO_HOME");
        assert_eq!(cfg.name, "TestHero");
        assert_eq!(cfg.pronoun, "they");
        assert_eq!(cfg.voice_default, "formal");
    }
}
