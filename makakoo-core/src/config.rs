//! Persona config loader.
//!
//! Reads `{makakoo_home}/config/persona.json`. the user's install stores
//! `{ "name": "Harvey", "pronoun": "he", "voice_default": "caveman" }` —
//! which matches the defaults returned when the file is absent.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::platform::makakoo_home;

/// Default persona name ("Harvey").
fn default_persona_name() -> String {
    "Harvey".to_string()
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
            pronoun: "he".to_string(),
            voice_default: default_voice_default(),
        }
    }
}

/// Load the persona config. Returns defaults if the file is missing or
/// cannot be parsed as JSON. T18 — serde defaults make any subset of
/// fields load cleanly, so a partial persona.json (only `name` + `user`,
/// the canonical production shape) deserialises silently without a warning.
pub fn load_persona() -> Result<PersonaConfig> {
    let path = makakoo_home().join("config").join("persona.json");
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
    fn default_is_harvey() {
        let p = PersonaConfig::default();
        assert_eq!(p.name, "Harvey");
        assert_eq!(p.pronoun, "he");
        assert_eq!(p.voice_default, "caveman");
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
