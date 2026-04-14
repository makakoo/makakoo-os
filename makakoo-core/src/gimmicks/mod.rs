//! Mascot gimmicks — LEGO-composed ASCII moments for Makakoo OS.
//!
//! Port of `core/terminal/gimmicks.py` + `core/terminal/lego_art.py`. The
//! Python modules are pinned by the T1 migration test oracle
//! (`tests/migration/test_gimmick_render.py`) and this Rust version preserves
//! every observable behavior:
//!
//! * `compose(species, context)` returns exactly 3 uniform-width lines;
//!   unknown species falls back to a random valid one, unknown context falls
//!   back to the `search` recipe.
//! * `render_gimmick(context, force)` is the MCP-safe non-TTY wrapper —
//!   **plain text, no ANSI** (MCP clients paste the output into response
//!   bubbles where escape codes leak).
//! * A 5-minute cooldown (per process) suppresses subsequent calls unless
//!   `force = true` or `render_forced` is used.
//! * `MAKAKOO_GIMMICK_OFF=1` (legacy alias: `HARVEY_GIMMICK_OFF`) disables the
//!   entire subsystem.
//!
//! The subsystem is intentionally zero-I/O: no nursery.json or buddy.json
//! lookups (those live in `makakoo-core::nursery`, T10). This module only
//! cares about the static LEGO catalog.

pub mod compositor;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::error::{MakakooError, Result};

pub use compositor::{
    all_recipes, all_species, compose, compose_for_mascot, compose_random,
    ACCESSORIES, BODIES, EYES, FEET, FLOATS, HEADS, RECIPES,
};

/// Cooldown between renders (per process) when not forced. 5 minutes matches
/// the Python `_MCP_COOLDOWN_SEC`.
pub const COOLDOWN: Duration = Duration::from_secs(300);

// ── Context → mascot source mapping ───────────────────────────
//
// Mirrors `CONTEXT_SOURCE` in gimmicks.py. The source is how the name is
// chosen — "olibia" is always the owl, "nursery" picks a random nursery
// mascot (stubbed here to "Nurselet"), "buddy" is the personal buddy
// (stubbed to "Buddy" until T10 wires real nursery state).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MascotSource {
    Olibia,
    Nursery,
    Buddy,
}

pub fn context_source(context: &str) -> MascotSource {
    match context {
        "search" => MascotSource::Buddy,
        "memory" => MascotSource::Olibia,
        "sancho" => MascotSource::Nursery,
        "dream" => MascotSource::Nursery,
        "celebrate" => MascotSource::Nursery,
        "error" => MascotSource::Olibia,
        "boot" => MascotSource::Buddy,
        _ => MascotSource::Nursery,
    }
}

/// Every context key that has one-liners in `GIMMICK_LINES`.
pub const CONTEXT_KEYS: &[&str] = &[
    "search",
    "memory",
    "sancho",
    "dream",
    "celebrate",
    "error",
    "boot",
];

/// Per-context one-liner templates. `{name}` is substituted with the mascot
/// name at render time. Mirrors `GIMMICK_LINES` in gimmicks.py.
pub const GIMMICK_LINES: &[(&str, &[&str])] = &[
    (
        "search",
        &[
            "{name} sniffs the knowledge graph...",
            "{name} digs through the Brain...",
            "shh... {name} is thinking...",
            "{name} found something!",
            "{name} peers into the archives...",
            "the Brain hums. {name} listens.",
            "{name} follows the wikilinks...",
            "{name} checks the indexes...",
        ],
    ),
    (
        "memory",
        &[
            "{name} watches over your memories",
            "{name} remembers so you don't have to",
            "another memory secured. {name} approves.",
            "{name} guards the promoted memories",
            "the owl sees all. {name} nods.",
            "{name}: memories are safe",
        ],
    ),
    (
        "sancho",
        &[
            "{name} woke up for this task",
            "{name} yawns... but gets to work",
            "{name} runs maintenance quietly",
            "{name} checks the schedule...",
            "{name} stretches and ticks",
            "{name} handles the boring stuff",
        ],
    ),
    (
        "dream",
        &[
            "{name} dreams of better clusters...",
            "zzz... {name} consolidates in sleep",
            "{name} mumbles about knowledge graphs...",
            "{name} turns over, dreaming of embeddings",
        ],
    ),
    (
        "celebrate",
        &[
            "{name} does a little dance!",
            "{name} is SO proud right now",
            "another win. {name} marks the occasion.",
            "{name} squeaks with delight!",
            "{name} beams quietly",
            "shipped. {name} noticed.",
        ],
    ),
    (
        "error",
        &[
            "{name} frowns at this...",
            "{name} tilts head. something's off.",
            "{name} stands watch. we'll fix this.",
        ],
    ),
    (
        "boot",
        &[
            "{name} stretches and yawns",
            "{name} is here. ready.",
            "{name} wakes up with Makakoo",
            "{name} blinks awake",
        ],
    ),
];

fn lines_for(context: &str) -> &'static [&'static str] {
    GIMMICK_LINES
        .iter()
        .find(|(k, _)| *k == context)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| {
            GIMMICK_LINES
                .iter()
                .find(|(k, _)| *k == "search")
                .map(|(_, v)| *v)
                .unwrap()
        })
}

/// A rendered mascot moment ready to hand back to the caller.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedGimmick {
    pub context: String,
    pub mascot_name: String,
    pub source: MascotSource,
    pub art: Vec<String>,
    pub one_liner: String,
    pub frame: String,
}

/// Species record for external inspection (registry `species()` accessors).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Species {
    pub name: String,
    pub head: String,
    pub body: String,
    pub feet: String,
}

/// Recipe record for external inspection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub eyes: String,
    pub accessory: String,
    pub float_key: String,
}

/// LEGO gimmick registry: holds the static species/recipe catalog and a
/// per-process cooldown map so renders honor the 5-minute MCP rate limit.
pub struct GimmickRegistry {
    species: HashMap<String, Species>,
    recipes: HashMap<String, Recipe>,
    cooldowns: Mutex<HashMap<String, Instant>>,
    off: bool,
}

static GLOBAL_REGISTRY: Lazy<GimmickRegistry> =
    Lazy::new(|| GimmickRegistry::load().expect("gimmick registry load"));

impl GimmickRegistry {
    /// Build the registry from the embedded LEGO catalog. Never does I/O, so
    /// the `Result` is really always `Ok`, but the signature keeps parity with
    /// other makakoo-core subsystems that do load files.
    pub fn load() -> Result<Self> {
        let mut species = HashMap::new();
        for (name, head) in HEADS {
            let body = BODIES
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| {
                    MakakooError::internal(format!("species {name} missing body entry"))
                })?;
            let feet = FEET
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| {
                    MakakooError::internal(format!("species {name} missing feet entry"))
                })?;
            species.insert(
                (*name).to_string(),
                Species {
                    name: (*name).to_string(),
                    head: (*head).to_string(),
                    body: body.to_string(),
                    feet: feet.to_string(),
                },
            );
        }

        let mut recipes = HashMap::new();
        for r in RECIPES {
            recipes.insert(
                r.name.to_string(),
                Recipe {
                    name: r.name.to_string(),
                    eyes: r.eyes.to_string(),
                    accessory: r.accessory.to_string(),
                    float_key: r.float_key.to_string(),
                },
            );
        }

        let off = std::env::var("MAKAKOO_GIMMICK_OFF")
            .ok()
            .or_else(|| std::env::var("HARVEY_GIMMICK_OFF").ok())
            .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(false);

        Ok(Self {
            species,
            recipes,
            cooldowns: Mutex::new(HashMap::new()),
            off,
        })
    }

    /// Shared process-wide registry. First access lazily initializes.
    pub fn global() -> &'static GimmickRegistry {
        &GLOBAL_REGISTRY
    }

    pub fn species(&self, name: &str) -> Option<&Species> {
        self.species.get(name)
    }

    pub fn recipe(&self, name: &str) -> Option<&Recipe> {
        self.recipes.get(name)
    }

    pub fn list_species(&self) -> Vec<&str> {
        self.species.keys().map(|s| s.as_str()).collect()
    }

    pub fn list_recipes(&self) -> Vec<&str> {
        self.recipes.keys().map(|s| s.as_str()).collect()
    }

    /// Return the canonical species count (18 in the Python oracle).
    pub fn species_count(&self) -> usize {
        self.species.len()
    }

    pub fn recipe_count(&self) -> usize {
        self.recipes.len()
    }

    fn cooldown_key(context: &str) -> String {
        // Python keys on (species, recipe) but render_gimmick only ever varies
        // by context within one process; we mirror the Python behavior and key
        // on the context string alone.
        context.to_string()
    }

    fn cooldown_ok(&self, context: &str) -> bool {
        let map = self.cooldowns.lock().unwrap();
        match map.get(&Self::cooldown_key(context)) {
            Some(last) => last.elapsed() >= COOLDOWN,
            None => true,
        }
    }

    fn cooldown_mark(&self, context: &str) {
        let mut map = self.cooldowns.lock().unwrap();
        map.insert(Self::cooldown_key(context), Instant::now());
    }

    /// Reset the cooldown map. Tests use this for isolation; production code
    /// never needs to call it.
    pub fn reset_cooldown(&self) {
        self.cooldowns.lock().unwrap().clear();
    }

    /// Validate that a species exists. Used by render_for_species.
    fn require_species(&self, name: &str) -> Result<()> {
        if self.species.contains_key(name) {
            Ok(())
        } else {
            Err(MakakooError::NotFound(format!("species: {name}")))
        }
    }

    fn require_recipe(&self, name: &str) -> Result<()> {
        if self.recipes.contains_key(name) {
            Ok(())
        } else {
            Err(MakakooError::NotFound(format!("recipe: {name}")))
        }
    }

    fn mascot_name(source: MascotSource) -> &'static str {
        match source {
            MascotSource::Olibia => "Olibia",
            MascotSource::Nursery => "Nurselet",
            MascotSource::Buddy => "Buddy",
        }
    }

    fn species_for_source(source: MascotSource) -> &'static str {
        match source {
            MascotSource::Olibia => "owl",
            MascotSource::Nursery => "fox",
            MascotSource::Buddy => "fox",
        }
    }

    /// Render a gimmick, honoring `_OFF` and the 5-minute cooldown.
    ///
    /// Returns `Ok(None)` when the cooldown is hot or the subsystem is off,
    /// `Ok(Some(frame))` on a successful render, `Err(_)` only on catastrophic
    /// internal failure (species/recipe lookup mismatch vs. the static tables).
    pub fn render(&self, context: &str) -> Result<Option<RenderedGimmick>> {
        if self.off {
            return Ok(None);
        }
        if !self.cooldown_ok(context) {
            return Ok(None);
        }
        let out = self.render_forced(context)?;
        self.cooldown_mark(context);
        Ok(Some(out))
    }

    /// Bypass the cooldown and always render. Used by `force=True` callers
    /// and by acceptance tests.
    pub fn render_forced(&self, context: &str) -> Result<RenderedGimmick> {
        if self.off {
            return Err(MakakooError::internal(
                "gimmick subsystem disabled by env flag",
            ));
        }

        let source = context_source(context);
        let mascot_name = Self::mascot_name(source);
        let species_key = Self::species_for_source(source);

        let art = compose(species_key, context);
        if art.len() != 3 {
            return Err(MakakooError::internal(format!(
                "compositor returned {} lines, expected 3",
                art.len()
            )));
        }

        let lines = lines_for(context);
        let mut rng = rand::thread_rng();
        let template = lines.choose(&mut rng).copied().unwrap_or("{name} is here.");
        let one_liner = template.replace("{name}", mascot_name);

        let frame = render_frame(&art, &one_liner);

        Ok(RenderedGimmick {
            context: context.to_string(),
            mascot_name: mascot_name.to_string(),
            source,
            art,
            one_liner,
            frame,
        })
    }

    /// Render using an explicit species + recipe pair. Unknown species or
    /// recipe yields `NotFound`. Bypasses the cooldown (used by tests and by
    /// direct `compose()` callers who already know their ids).
    pub fn render_species_recipe(
        &self,
        species: &str,
        recipe: &str,
    ) -> Result<RenderedGimmick> {
        self.require_species(species)?;
        self.require_recipe(recipe)?;

        let art = compose(species, recipe);
        let mascot_name = "Olibia"; // arbitrary stable name for the low-level API
        let lines = lines_for(recipe);
        let mut rng = rand::thread_rng();
        let template = lines.choose(&mut rng).copied().unwrap_or("{name} is here.");
        let one_liner = template.replace("{name}", mascot_name);
        let frame = render_frame(&art, &one_liner);

        Ok(RenderedGimmick {
            context: recipe.to_string(),
            mascot_name: mascot_name.to_string(),
            source: MascotSource::Olibia,
            art,
            one_liner,
            frame,
        })
    }
}

/// Assemble the 3-line art and one-liner into a final frame. Mirrors
/// `_render` in gimmicks.py for the plain-text (non-TTY) branch — MCP
/// consumers get this output, so there are **no ANSI escape codes**.
fn render_frame(art: &[String], one_liner: &str) -> String {
    let max_width = art.iter().map(|l| l.chars().count()).max().unwrap_or(10);
    let gap = "  ";
    let mut out = String::from("\n");
    for (i, line) in art.iter().enumerate() {
        // Normalize just in case the caller handed us unpadded lines.
        let width = line.chars().count();
        let mut padded = line.clone();
        if width < max_width {
            for _ in 0..(max_width - width) {
                padded.push(' ');
            }
        }

        if i == 1 {
            out.push_str(&padded);
            out.push_str(gap);
            out.push_str(one_liner);
        } else {
            out.push_str(&padded);
        }
        out.push('\n');
    }
    out
}

/// Top-level convenience: maybe render a mascot moment for `context`.
///
/// Returns `Ok(None)` if the 5-minute cooldown is still hot, the subsystem
/// is disabled, or the context has no associated one-liner pool. `force=true`
/// bypasses the cooldown but still respects the `_OFF` kill switch, matching
/// the Python oracle.
pub fn render_gimmick(context: &str, force: bool) -> Result<Option<String>> {
    let reg = GimmickRegistry::global();
    if force {
        if reg.off {
            return Ok(None);
        }
        let out = reg.render_forced(context)?;
        reg.cooldown_mark(context);
        return Ok(Some(out.frame));
    }
    Ok(reg.render(context)?.map(|g| g.frame))
}

// ══════════════════════════════════════════════════════════════
//  Unit tests — mirror the T1 Python acceptance oracle.
// ══════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fresh (non-global) registry so tests don't race on the shared
    /// cooldown map.
    fn fresh() -> GimmickRegistry {
        GimmickRegistry::load().expect("load")
    }

    #[test]
    fn species_catalog_loads_non_empty() {
        let reg = fresh();
        assert!(reg.species_count() > 0);
        assert_eq!(reg.species_count(), 18, "Python oracle has 18 species");
    }

    #[test]
    fn recipe_catalog_loads_non_empty() {
        let reg = fresh();
        assert!(reg.recipe_count() > 0);
        assert_eq!(reg.recipe_count(), 9, "Python oracle has 9 recipes");
    }

    #[test]
    fn every_species_has_head_body_feet() {
        for sp in all_species() {
            assert!(HEADS.iter().any(|(k, _)| *k == sp), "missing head: {sp}");
            assert!(BODIES.iter().any(|(k, _)| *k == sp), "missing body: {sp}");
            assert!(FEET.iter().any(|(k, _)| *k == sp), "missing feet: {sp}");
        }
    }

    #[test]
    fn every_recipe_references_valid_pools() {
        for r in RECIPES {
            assert!(
                EYES.iter().any(|(k, _)| *k == r.eyes),
                "recipe {} references missing eyes pool {}",
                r.name,
                r.eyes
            );
            assert!(
                ACCESSORIES.iter().any(|(k, _)| *k == r.accessory),
                "recipe {} references missing accessory pool {}",
                r.name,
                r.accessory
            );
            assert!(
                FLOATS.iter().any(|(k, _)| *k == r.float_key),
                "recipe {} references missing float pool {}",
                r.name,
                r.float_key
            );
        }
    }

    #[test]
    fn compose_returns_three_lines() {
        let lines = compose("fox", "search");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn compose_uniform_width() {
        let lines = compose("owl", "memory");
        let widths: std::collections::HashSet<usize> =
            lines.iter().map(|l| l.chars().count()).collect();
        assert_eq!(widths.len(), 1, "non-uniform widths: {:?}", widths);
    }

    #[test]
    fn compose_every_species_every_context() {
        for sp in all_species() {
            for ctx in all_recipes() {
                let lines = compose(sp, ctx);
                assert_eq!(lines.len(), 3, "species {sp} ctx {ctx}");
                let widths: std::collections::HashSet<usize> =
                    lines.iter().map(|l| l.chars().count()).collect();
                assert_eq!(widths.len(), 1, "species {sp} ctx {ctx} non-uniform");
            }
        }
    }

    #[test]
    fn compose_unknown_species_fallback() {
        // Must not panic — falls back to a random valid species.
        let lines = compose("not_a_real_species", "search");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn compose_unknown_context_falls_back_to_search() {
        let lines = compose("fox", "definitely_not_a_context");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn compose_random_returns_species_name() {
        let (lines, species) = compose_random("search");
        assert_eq!(lines.len(), 3);
        assert!(all_species().contains(&species));
    }

    #[test]
    fn compose_for_mascot_stable_width() {
        let a = compose_for_mascot("Olibia", "owl", "memory");
        let b = compose_for_mascot("Olibia", "owl", "memory");
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 3);
        assert_eq!(a[0].chars().count(), b[0].chars().count());
    }

    #[test]
    fn render_forced_returns_non_empty_frame() {
        let reg = fresh();
        let out = reg.render_forced("memory").expect("render");
        assert!(!out.frame.is_empty());
        // frame = "\n" + 3 art lines (each with trailing \n) → at least 4 newlines.
        assert!(out.frame.matches('\n').count() >= 4);
    }

    #[test]
    fn render_gimmick_force_bypasses_cooldown() {
        let reg = fresh();
        let first = reg.render_forced("memory").expect("first");
        let second = reg.render_forced("memory").expect("second");
        assert!(!first.frame.is_empty());
        assert!(!second.frame.is_empty());
    }

    #[test]
    fn render_cooldown_enforced() {
        let reg = fresh();
        let first = reg.render("memory").expect("first").expect("Some");
        assert!(!first.frame.is_empty());
        // Second call within cooldown window → None.
        let second = reg.render("memory").expect("second");
        assert!(second.is_none(), "cooldown failed: {:?}", second);
    }

    #[test]
    fn render_cooldown_resets() {
        let reg = fresh();
        let _ = reg.render("memory").unwrap();
        reg.reset_cooldown();
        let second = reg.render("memory").expect("second");
        assert!(second.is_some());
    }

    #[test]
    fn render_forced_plain_text_no_ansi() {
        // MCP consumers read this output. No ANSI escape sequences allowed.
        let reg = fresh();
        let out = reg.render_forced("celebrate").expect("render");
        assert!(
            !out.frame.contains('\x1b'),
            "ANSI leaked into MCP output: {:?}",
            out.frame
        );
    }

    #[test]
    fn render_species_recipe_unknown_species_errors() {
        let reg = fresh();
        let result = reg.render_species_recipe("not_a_species", "search");
        assert!(matches!(result, Err(MakakooError::NotFound(_))));
    }

    #[test]
    fn render_species_recipe_unknown_recipe_errors() {
        let reg = fresh();
        let result = reg.render_species_recipe("fox", "not_a_recipe");
        assert!(matches!(result, Err(MakakooError::NotFound(_))));
    }

    #[test]
    fn render_species_recipe_known_pair_ok() {
        let reg = fresh();
        let out = reg.render_species_recipe("fox", "search").expect("ok");
        assert_eq!(out.art.len(), 3);
    }

    #[test]
    fn list_species_matches_count() {
        let reg = fresh();
        assert_eq!(reg.list_species().len(), reg.species_count());
    }

    #[test]
    fn list_recipes_matches_count() {
        let reg = fresh();
        assert_eq!(reg.list_recipes().len(), reg.recipe_count());
    }

    #[test]
    fn every_context_source_has_lines() {
        // Mirrors Python test_gimmick_lines_all_contexts_have_pools.
        for ctx in CONTEXT_KEYS {
            let lines = lines_for(ctx);
            assert!(!lines.is_empty(), "context {ctx} has no one-liners");
        }
    }

    #[test]
    fn off_env_disables_render() {
        let reg = GimmickRegistry {
            species: GimmickRegistry::load().unwrap().species,
            recipes: GimmickRegistry::load().unwrap().recipes,
            cooldowns: Mutex::new(HashMap::new()),
            off: true,
        };
        assert!(reg.render("memory").unwrap().is_none());
    }
}
