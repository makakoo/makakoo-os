//! LD#16 untrusted-field escape helper.
//!
//! Mirrors Python `core.capability.user_grants.escape_audit_field`.
//! Both implementations exercise the shared fixture at
//! `tests/fixtures/audit_escape_vectors.json`; drift fails CI.
//!
//! Used before writing `label` and `plugin` fields to the audit log
//! (JSONL) or rendering them in CLI output. Strips C0/C1 control
//! chars + DEL, converts whitespace-ish controls (`\n \r \t \v \f`)
//! to literal spaces so that `split_whitespace().collect(" ")` then
//! collapses wordbreaks without fusing adjacent tokens, and truncates
//! to `max_len` chars with a trailing ellipsis.

/// Escape + truncate a user-supplied string for safe audit emission.
/// See module-level doc for behavior rationale.
pub fn escape_audit_field(s: &str, max_len: usize) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut cleaned = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp == 0x7F {
            continue;
        }
        if cp < 0x20 {
            if matches!(ch, '\n' | '\r' | '\t' | '\x0B' | '\x0C') {
                cleaned.push(' ');
            }
            // Other C0 controls silently dropped.
            continue;
        }
        if (0x80..=0x9F).contains(&cp) {
            continue;
        }
        cleaned.push(ch);
    }

    // Collapse whitespace runs to single spaces.
    let cleaned: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    // Truncate with ellipsis character '…' (1 char), respecting char
    // boundaries not byte boundaries.
    let char_count = cleaned.chars().count();
    if char_count <= max_len {
        cleaned
    } else if max_len == 0 {
        String::new()
    } else {
        let mut out: String = cleaned.chars().take(max_len - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_fixture_round_trip() {
        let fixture = include_str!(
            "../../../tests/fixtures/audit_escape_vectors.json"
        );
        let v: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let vectors = v["vectors"].as_array().expect("vectors array");
        let mut failures: Vec<String> = Vec::new();
        for entry in vectors {
            let name = entry["name"].as_str().unwrap_or("<unnamed>");
            let input = entry["input"].as_str().unwrap_or("");
            let max_len = entry["max_len"].as_u64().unwrap_or(80) as usize;
            let expected = entry["output"].as_str().unwrap_or("");
            let actual = escape_audit_field(input, max_len);
            if actual != expected {
                failures.push(format!(
                    "{name}: input={input:?} max_len={max_len} → {actual:?} \
                     (expected {expected:?})"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "shared-fixture drift ({} cases):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(escape_audit_field("", 80), "");
    }

    #[test]
    fn truncation_uses_ellipsis_char() {
        let s = "a".repeat(200);
        let out = escape_audit_field(&s, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn unicode_printable_passthrough() {
        let s = "résumé naïve café";
        assert_eq!(escape_audit_field(s, 80), s);
    }
}
