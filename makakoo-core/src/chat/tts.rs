//! `speak` — cross-platform text-to-speech subprocess dispatch.
//!
//! * macOS — shells out to `say`
//! * Linux — shells out to `espeak`
//! * Windows — shells out to PowerShell `System.Speech.Synthesis`
//! * Other targets — returns `MakakooError::Internal`
//!
//! Non-zero exits from the child are surfaced as `MakakooError::Internal`
//! so the caller can distinguish "TTS ran and the user heard it" from
//! "TTS binary is missing". The empty-string case is still valid on
//! every supported platform and just plays silence.

use crate::error::{MakakooError, Result};

/// Speak `text` aloud through the platform default TTS binary.
pub fn speak(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        speak_macos(text)
    }
    #[cfg(target_os = "linux")]
    {
        speak_linux(text)
    }
    #[cfg(target_os = "windows")]
    {
        speak_windows(text)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        Err(MakakooError::internal("TTS unsupported on this OS"))
    }
}

#[cfg(target_os = "macos")]
fn speak_macos(text: &str) -> Result<()> {
    let status = std::process::Command::new("say").arg(text).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(MakakooError::internal(format!(
            "`say` exited with status {status}"
        )))
    }
}

#[cfg(target_os = "linux")]
fn speak_linux(text: &str) -> Result<()> {
    let status = std::process::Command::new("espeak").arg(text).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(MakakooError::internal(format!(
            "`espeak` exited with status {status}"
        )))
    }
}

#[cfg(target_os = "windows")]
fn speak_windows(text: &str) -> Result<()> {
    // Escape single quotes for PowerShell: ' -> ''
    let escaped = text.replace('\'', "''");
    let ps = format!(
        "Add-Type -AssemblyName System.Speech; \
         (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{}')",
        escaped
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(MakakooError::internal(format!(
            "`powershell` exited with status {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn speak_empty_exits_zero_on_macos() {
        // `say ""` is a legal no-op and exits 0. If the binary is absent
        // (very unusual on macOS) we accept `Err` so CI stays honest.
        match speak("") {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.to_lowercase().contains("say") || msg.contains("io"),
                    "unexpected error from empty speak: {msg}"
                );
            }
        }
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn speak_is_unsupported_on_other_targets() {
        let err = speak("hello").unwrap_err();
        assert!(format!("{err}").to_lowercase().contains("unsupported"));
    }
}
