//! `makakoo version` — print persona, version, git hash.

use makakoo_core::config::load_persona;

const GIT_HASH: Option<&str> = option_env!("MAKAKOO_GIT_HASH");

pub fn run() -> anyhow::Result<i32> {
    let pkg_version = env!("CARGO_PKG_VERSION");
    let git = GIT_HASH.unwrap_or("unknown");
    let persona = load_persona().unwrap_or_default();
    println!("makakoo {pkg_version} ({git})");
    println!(
        "persona: {} (pronoun={}, voice_default={})",
        persona.name, persona.pronoun, persona.voice_default
    );
    println!("home: {}", makakoo_core::platform::makakoo_home().display());
    Ok(0)
}
