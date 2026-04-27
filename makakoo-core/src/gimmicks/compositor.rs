//! LEGO compositor — assembles 3-line ASCII mascots from species + context parts.
//!
//! Mirrors `core/terminal/lego_art.py`. The Python module is the behavioral oracle
//! pinned by the T1 migration test suite, so the Rust port preserves every part
//! table byte-for-byte. Eyes, accessories, and floats are pools — one glyph is
//! picked at random per compose() call, so two calls with the same inputs may
//! produce different art (but always 3 lines of uniform width).
//!
//! Structure:
//!   HEADS[species]  -> top line
//!   BODIES[species] -> middle line template with {E} placeholder
//!   FEET[species]   -> bottom line
//!   EYES[mood]      -> pool of 3-char eye expressions replacing {E}
//!   ACCESSORIES[k]  -> pool of right-side glyphs appended to the body line
//!   FLOATS[k]       -> pool of top-line trailing glyphs (thought bubbles etc)
//!   RECIPES[ctx]    -> maps a context ("search", "memory", ...) to (eyes, accessory, float) pool names

use rand::seq::SliceRandom;

/// A single species definition — the three body parts driven by species name.
#[derive(Clone, Debug)]
pub struct SpeciesArt {
    pub name: &'static str,
    pub head: &'static str,
    pub body: &'static str, // contains "{E}" placeholder
    pub feet: &'static str,
}

/// A context recipe — picks which eye/accessory/float pool to draw from.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub name: &'static str,
    pub eyes: &'static str,
    pub accessory: &'static str,
    pub float_key: &'static str,
}

// ── Heads ──────────────────────────────────────────────────────
pub const HEADS: &[(&str, &str)] = &[
    ("fox", "  /\\_/\\  "),
    ("cat", "  /\\_/\\  "),
    ("wolf", " /\\___/\\ "),
    ("panther", "  /\\_/\\  "),
    ("owl", "  {___}  "),
    ("raven", "  \\___/  "),
    ("eagle", "  \\v v/  "),
    ("hawk", "  \\\\v//  "),
    ("bat", " /\\\\_/\\\\ "),
    ("bear", " (\")(\")"),
    ("octopus", "  ,--,   "),
    ("dragon", "  /\\_/\\~ "),
    ("phoenix", "  \\|/|/  "),
    ("shark", "  ___/\\  "),
    ("spider", " /\\ /\\  "),
    ("snake", "  _____  "),
    ("scorpion", "  __/~   "),
    ("kraken", "  \\   /  "),
];

// ── Bodies (with {E} placeholder) ─────────────────────────────
pub const BODIES: &[(&str, &str)] = &[
    ("fox", " ( {E} ) "),
    ("cat", " ( {E} ) "),
    ("wolf", " ( {E} )"),
    ("panther", " ( {E} ) "),
    ("owl", "  |){E}| "),
    ("raven", "  ({E})  "),
    ("eagle", "  ({E})  "),
    ("hawk", "  ({E})  "),
    ("bat", "  ({E})  "),
    ("bear", " ({E} ) "),
    ("octopus", " ({E} ) "),
    ("dragon", " ( {E} ) "),
    ("phoenix", "  ({E})  "),
    ("shark", " ({E}  >)"),
    ("spider", " \\({E})/ "),
    ("snake", " /~{E}~\\ "),
    ("scorpion", " ({E} ) "),
    ("kraken", " \\({E})/ "),
];

// ── Feet ───────────────────────────────────────────────────────
pub const FEET: &[(&str, &str)] = &[
    ("fox", "  > ^ <  "),
    ("cat", "  (\")_(\") "),
    ("wolf", "  U     U "),
    ("panther", "  ( u u) "),
    ("owl", "  -\"-\"-  "),
    ("raven", "  //^\\\\  "),
    ("eagle", "  _/|\\_  "),
    ("hawk", "  _/|\\_  "),
    ("bat", "  \\/ \\/  "),
    ("bear", "  (oo)   "),
    ("octopus", " /|/|\\|\\ "),
    ("dragon", "  ~\\/\\/  "),
    ("phoenix", "  /|\\|\\  "),
    ("shark", "  ---\\/  "),
    ("spider", " /|/ \\|\\ "),
    ("snake", " \\_____/ "),
    ("scorpion", "  \\___/\\ "),
    ("kraken", " /|/|\\|\\ "),
];

// ── Eye pools ──────────────────────────────────────────────────
pub const EYES: &[(&str, &[&str])] = &[
    ("neutral", &["o.o", "o_o", "·.·"]),
    ("happy", &["^.^", "^_^", "^·^"]),
    ("thinking", &["o.o", "·.·", "°.°"]),
    ("sleepy", &["-.-", "-.~", "~.~"]),
    ("excited", &["*.* ", "!.!", "°o°"]),
    ("worried", &["o.O", "O.o", "°.O"]),
    ("proud", &["^.~", "^.o", "~.^"]),
    ("searching", &[">.>", ">.o", "o.<"]),
    ("dreamy", &["~.~", "-.~", "~.-"]),
    ("alert", &["O.O", "O_O", "0.0"]),
    ("mischief", &[">.^", "^.<", ">.~"]),
    ("love", &["♥.♥", "❤.❤", "♡.♡"]),
];

// ── Accessory pools ────────────────────────────────────────────
pub const ACCESSORIES: &[(&str, &[&str])] = &[
    ("magnifying_glass", &["~Q", "oQ", "-Q"]),
    ("book", &["[=]", "[#]", "[-]"]),
    ("shield", &["|]", "[>", "|>"]),
    ("wrench", &["}>", "d-", "|>"]),
    ("zzz", &["z z", "Z z", "z Z"]),
    ("sparkle", &["* *", "✦ ✦", "✧ ✧"]),
    ("heart", &["<3", "♥", "♡"]),
    ("exclaim", &[" !", "!!", " !"]),
    ("question", &[" ?", "??", " ?"]),
    ("coffee", &["c[_]", "c(_)", "[_]>"]),
    ("scroll", &["~=", "~≡", "=~"]),
    ("flag", &["|>", "|►", "|▸"]),
    ("none", &["", "", ""]),
];

// ── Float pools ────────────────────────────────────────────────
pub const FLOATS: &[(&str, &[&str])] = &[
    ("thought", &["...", " . ", "···"]),
    ("zzz", &["z Z", "Z z", "zZz"]),
    ("sparkle", &["✦", "✧", "*"]),
    ("exclaim", &["!", "!!", "⚡"]),
    ("heart", &["♥", "❤", "♡"]),
    ("music", &["♪", "♫", "♬"]),
    ("sweat", &["'", ";;", "\""]),
    ("none", &["", "", ""]),
];

// ── Recipes ────────────────────────────────────────────────────
pub const RECIPES: &[Recipe] = &[
    Recipe {
        name: "search",
        eyes: "searching",
        accessory: "magnifying_glass",
        float_key: "thought",
    },
    Recipe {
        name: "memory",
        eyes: "proud",
        accessory: "book",
        float_key: "sparkle",
    },
    Recipe {
        name: "sancho",
        eyes: "sleepy",
        accessory: "wrench",
        float_key: "none",
    },
    Recipe {
        name: "dream",
        eyes: "dreamy",
        accessory: "none",
        float_key: "zzz",
    },
    Recipe {
        name: "celebrate",
        eyes: "excited",
        accessory: "sparkle",
        float_key: "music",
    },
    Recipe {
        name: "error",
        eyes: "worried",
        accessory: "exclaim",
        float_key: "sweat",
    },
    Recipe {
        name: "boot",
        eyes: "neutral",
        accessory: "coffee",
        float_key: "none",
    },
    Recipe {
        name: "promote",
        eyes: "proud",
        accessory: "flag",
        float_key: "sparkle",
    },
    Recipe {
        name: "consolidate",
        eyes: "thinking",
        accessory: "scroll",
        float_key: "thought",
    },
];

// ── Helpers ────────────────────────────────────────────────────

pub fn all_species() -> Vec<&'static str> {
    HEADS.iter().map(|(k, _)| *k).collect()
}

pub fn all_recipes() -> Vec<&'static str> {
    RECIPES.iter().map(|r| r.name).collect()
}

fn lookup<'a>(table: &'a [(&'static str, &'static str)], key: &str) -> Option<&'a str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn lookup_pool<'a>(
    table: &'a [(&'static str, &'static [&'static str])],
    key: &str,
) -> Option<&'a [&'static str]> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn recipe_for(context: &str) -> &'static Recipe {
    RECIPES
        .iter()
        .find(|r| r.name == context)
        .unwrap_or_else(|| RECIPES.iter().find(|r| r.name == "search").unwrap())
}

fn pick<'a, R: rand::Rng>(pool: &'a [&'static str], rng: &mut R) -> &'a str {
    pool.choose(rng).copied().unwrap_or("")
}

/// Normalize an iterator of strings to uniform width by right-padding with spaces.
fn normalize_widths(lines: Vec<String>) -> Vec<String> {
    let max_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    lines
        .into_iter()
        .map(|l| {
            let width = l.chars().count();
            if width >= max_w {
                l
            } else {
                let mut padded = l;
                for _ in 0..(max_w - width) {
                    padded.push(' ');
                }
                padded
            }
        })
        .collect()
}

/// Compose a 3-line ASCII mascot for `species` in `context`.
///
/// Unknown species falls back to a random valid species. Unknown context falls
/// back to the `search` recipe. The returned Vec is always length 3 and every
/// line has identical `.chars().count()`.
pub fn compose(species: &str, context: &str) -> Vec<String> {
    let mut rng = rand::thread_rng();
    compose_with_rng(species, context, &mut rng)
}

/// Deterministic variant used by tests — caller supplies the RNG.
pub fn compose_with_rng<R: rand::Rng>(
    species: &str,
    context: &str,
    rng: &mut R,
) -> Vec<String> {
    // Species fallback: unknown → random valid one.
    let species_key: &str = if lookup(HEADS, species).is_some() {
        // We need a &'static str so we pass the canonical constant string.
        HEADS
            .iter()
            .find(|(k, _)| *k == species)
            .map(|(k, _)| *k)
            .unwrap()
    } else {
        let all: Vec<&'static str> = HEADS.iter().map(|(k, _)| *k).collect();
        all.choose(rng).copied().unwrap_or("fox")
    };

    let recipe = recipe_for(context);

    let head = lookup(HEADS, species_key).unwrap_or_else(|| lookup(HEADS, "fox").unwrap());
    let body_template =
        lookup(BODIES, species_key).unwrap_or_else(|| lookup(BODIES, "fox").unwrap());
    let feet = lookup(FEET, species_key).unwrap_or_else(|| lookup(FEET, "fox").unwrap());

    let eye_pool =
        lookup_pool(EYES, recipe.eyes).unwrap_or_else(|| lookup_pool(EYES, "neutral").unwrap());
    let eyes_glyph = pick(eye_pool, rng);

    let body = body_template.replace("{E}", eyes_glyph);

    let acc_pool = lookup_pool(ACCESSORIES, recipe.accessory)
        .unwrap_or_else(|| lookup_pool(ACCESSORIES, "none").unwrap());
    let accessory = pick(acc_pool, rng);

    let float_pool = lookup_pool(FLOATS, recipe.float_key)
        .unwrap_or_else(|| lookup_pool(FLOATS, "none").unwrap());
    let float_char = pick(float_pool, rng);

    // Line 0: head [+ float]
    let line0 = if !float_char.is_empty() {
        format!("{} {}", head, float_char)
    } else {
        head.to_string()
    };

    // Line 1: body [+ accessory]
    let line1 = if !accessory.is_empty() {
        format!("{}{}", body, accessory)
    } else {
        body
    };

    // Line 2: feet
    let line2 = feet.to_string();

    normalize_widths(vec![line0, line1, line2])
}

/// Compose with a random species. Returns (lines, species_name).
pub fn compose_random(context: &str) -> (Vec<String>, &'static str) {
    let mut rng = rand::thread_rng();
    let all: Vec<&'static str> = HEADS.iter().map(|(k, _)| *k).collect();
    let species = all.choose(&mut rng).copied().unwrap_or("fox");
    (compose_with_rng(species, context, &mut rng), species)
}

/// Compose for a named mascot (nursery or buddy). Maps mascot species to the
/// closest base species by substring match, else falls back to a stable hash
/// of the mascot name.
pub fn compose_for_mascot(
    mascot_name: &str,
    mascot_species: &str,
    context: &str,
) -> Vec<String> {
    let species_lower = mascot_species.to_lowercase();
    let all: Vec<&'static str> = HEADS.iter().map(|(k, _)| *k).collect();

    for base in &all {
        if species_lower.contains(base) {
            return compose(base, context);
        }
    }

    let idx: usize = mascot_name.bytes().map(|b| b as usize).sum::<usize>() % all.len();
    compose(all[idx], context)
}
