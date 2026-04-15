//! `makakoo setup` — interactive first-run wizard.
//!
//! Asks the user to name their assistant (5 suggested + custom), pick a
//! pronoun, and choose a voice default. Writes the result to
//! `$MAKAKOO_HOME/config/persona.json`.
//!
//! By default the command refuses to clobber an existing persona file —
//! pass `--force` to re-run the wizard and overwrite. This keeps the
//! "accidentally nuked my config" blast radius at zero.
//!
//! The wizard's interactive loop is intentionally thin. The real logic
//! (parsing the numeric choice, resolving the custom name) lives in
//! [`makakoo_core::config::resolve_name_choice`] and is unit-tested
//! there, so this file only owns the I/O seams.

use std::io::{BufRead, Write};

use makakoo_core::config::{
    persona_path, resolve_name_choice, PersonaConfig, SUGGESTED_NAMES,
};

pub fn run(force: bool) -> anyhow::Result<i32> {
    let path = persona_path();
    if path.exists() && !force {
        eprintln!("persona already configured at {}", path.display());
        eprintln!("re-run with `makakoo setup --force` to overwrite.");
        return Ok(1);
    }

    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    banner(&mut stdout)?;
    let name = prompt_name(&mut stdin, &mut stdout)?;
    let pronoun = prompt_pronoun(&mut stdin, &mut stdout)?;
    let voice = prompt_voice(&mut stdin, &mut stdout)?;

    let cfg = PersonaConfig {
        name: name.clone(),
        pronoun,
        voice_default: voice,
    };
    cfg.save_to(&path)?;

    writeln!(stdout)?;
    writeln!(stdout, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    writeln!(stdout, "  Saved to {}", path.display())?;
    writeln!(stdout, "  Hi. I'm {name}. Let's build something.")?;
    writeln!(stdout, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    Ok(0)
}

fn banner<W: Write>(out: &mut W) -> anyhow::Result<()> {
    writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    writeln!(out, "  Welcome to Makakoo OS")?;
    writeln!(out, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    writeln!(out)?;
    writeln!(out, "Let's name your assistant. Pick a suggestion or")?;
    writeln!(out, "type your own — this is the name you'll call out")?;
    writeln!(out, "to in chat, in logs, and in the Brain.")?;
    writeln!(out)?;
    Ok(())
}

fn prompt_name<R: BufRead, W: Write>(stdin: &mut R, out: &mut W) -> anyhow::Result<String> {
    loop {
        writeln!(out, "Name suggestions:")?;
        for (i, name) in SUGGESTED_NAMES.iter().enumerate() {
            writeln!(out, "  {}. {}", i + 1, name)?;
        }
        writeln!(out, "  {}. (type your own)", SUGGESTED_NAMES.len() + 1)?;
        write!(out, "\nPick 1-{}: ", SUGGESTED_NAMES.len() + 1)?;
        out.flush()?;

        let choice = read_line(stdin)?;
        let custom = if choice.trim() == format!("{}", SUGGESTED_NAMES.len() + 1) {
            write!(out, "Custom name: ")?;
            out.flush()?;
            Some(read_line(stdin)?)
        } else {
            None
        };

        match resolve_name_choice(&choice, custom.as_deref()) {
            Some(name) => return Ok(name),
            None => {
                writeln!(out, "(didn't catch that — try again)\n")?;
                continue;
            }
        }
    }
}

fn prompt_pronoun<R: BufRead, W: Write>(stdin: &mut R, out: &mut W) -> anyhow::Result<String> {
    writeln!(out)?;
    write!(out, "Pronoun (he / she / they) [they]: ")?;
    out.flush()?;
    let raw = read_line(stdin)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok("they".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn prompt_voice<R: BufRead, W: Write>(stdin: &mut R, out: &mut W) -> anyhow::Result<String> {
    writeln!(out)?;
    writeln!(out, "Voice default:")?;
    writeln!(out, "  1. caveman  (terse, token-efficient, default)")?;
    writeln!(out, "  2. full     (formal prose, longer answers)")?;
    write!(out, "Pick 1-2 [1]: ")?;
    out.flush()?;
    let raw = read_line(stdin)?;
    match raw.trim() {
        "" | "1" => Ok("caveman".to_string()),
        "2" => Ok("full".to_string()),
        // Anything else we preserve verbatim — power users can write a
        // custom voice mode name and hook it into their own prompt.
        other => Ok(other.to_string()),
    }
}

fn read_line<R: BufRead>(stdin: &mut R) -> anyhow::Result<String> {
    let mut buf = String::new();
    let bytes = stdin.read_line(&mut buf)?;
    if bytes == 0 {
        // EOF during onboarding — bail cleanly instead of spinning.
        anyhow::bail!("setup: unexpected end of input");
    }
    Ok(buf)
}
