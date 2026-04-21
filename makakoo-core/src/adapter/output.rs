//! Output parsers — `Bytes` → `ValidatorResult`.
//!
//! Keyed off `manifest.output.format`. Four variants:
//!
//! - `lope-verdict-block` — find `---VERDICT---\n…\n---END---`, parse JSON
//!   first, fall back to a YAML-ish regex matching lope's legacy contract.
//! - `openai-chat` — extract `verdict_field` via dot-path, then recursively
//!   apply the lope-verdict-block parser. On no-block, synthesize a
//!   heuristic PASS+0.5 with the content as rationale.
//! - `plain` — full-text heuristic: grep for "pass"/"fail"/"needs fix",
//!   default PASS+0.5.
//! - `custom` — escape hatch: Phase B returns an explicit error so callers
//!   know they need to plug a Python parser in via Phase E.

use std::time::Duration;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use super::manifest::{Manifest, OutputFormat};
use super::result::{PhaseVerdict, ValidatorResult, VerdictStatus};

static VERDICT_BLOCK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)---\s*VERDICT\s*---(?P<body>.*?)---\s*END\s*---").expect("valid regex")
});

static STATUS_LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bstatus\s*:\s*(?P<val>PASS|NEEDS_FIX|FAIL|INFRA_ERROR)\b")
        .expect("valid regex")
});

static CONFIDENCE_LINE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bconfidence\s*:\s*(?P<val>[0-9.]+)").expect("valid regex"));

static RATIONALE_LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?is)\brationale\s*:\s*(?P<val>.+?)(?:\n[a-z_]+\s*:|\z)").expect("valid regex")
});

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("output format `custom` requires a Python parser (see Phase E consumer integration)")]
    CustomUnsupported,
    #[error("openai-chat response JSON missing verdict_field `{0}`")]
    VerdictFieldMissing(String),
    #[error("response body is not valid UTF-8: {0}")]
    NotUtf8(#[from] std::str::Utf8Error),
    #[error("openai-chat response is not valid JSON: {0}")]
    NotJson(String),
}

/// Entry point — parse a transport response into a verdict.
pub fn parse_response(
    manifest: &Manifest,
    body: &[u8],
    duration: Duration,
) -> Result<ValidatorResult, OutputError> {
    let text = std::str::from_utf8(body)?;
    let validator = manifest.adapter.name.as_str();

    match manifest.output.format {
        OutputFormat::LopeVerdictBlock => Ok(parse_verdict_block(
            validator, text, duration,
        )),
        OutputFormat::OpenAiChat => parse_openai_chat(manifest, text, duration),
        OutputFormat::Plain => Ok(parse_plain(validator, text, duration)),
        OutputFormat::Custom => Err(OutputError::CustomUnsupported),
    }
}

/// Parse a verdict block. Recognizes both JSON-inside-block (new) and the
/// YAML-ish legacy shape. Falls back to a plain-text heuristic when no
/// block is found so validators without VERDICT contracts still produce
/// low-confidence, non-fatal results.
fn parse_verdict_block(validator: &str, text: &str, duration: Duration) -> ValidatorResult {
    let secs = duration.as_secs_f64();

    let Some(cap) = VERDICT_BLOCK_RE.captures(text) else {
        // No block at all → plain heuristic with max-conf 0.5.
        return heuristic_result(validator, text, duration);
    };
    let block = cap.name("body").map(|m| m.as_str()).unwrap_or("").trim();

    // First try JSON.
    if let Ok(v) = serde_json::from_str::<Value>(block) {
        if let Some(r) = verdict_from_json(validator, &v, secs) {
            return wrap(validator, r, text);
        }
    }

    // YAML-ish fallback: scan line by line.
    let status = STATUS_LINE_RE
        .captures(block)
        .and_then(|c| c.name("val"))
        .and_then(|m| VerdictStatus::from_str(m.as_str()))
        .unwrap_or(VerdictStatus::NeedsFix);
    let confidence = CONFIDENCE_LINE_RE
        .captures(block)
        .and_then(|c| c.name("val"))
        .and_then(|m| m.as_str().parse::<f64>().ok())
        .unwrap_or(0.5);
    let rationale = RATIONALE_LINE_RE
        .captures(block)
        .and_then(|c| c.name("val"))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_else(|| block.trim().to_string());

    wrap(
        validator,
        PhaseVerdict {
            status,
            confidence: clamp01(confidence),
            rationale,
            required_fixes: Vec::new(),
            nice_to_have: Vec::new(),
            duration_seconds: secs,
            validator_name: validator.to_string(),
            stage: None,
            evidence_gate_triggered: false,
        },
        text,
    )
}

fn verdict_from_json(validator: &str, v: &Value, duration_seconds: f64) -> Option<PhaseVerdict> {
    let obj = v.as_object()?;
    let status = obj
        .get("status")
        .and_then(|s| s.as_str())
        .and_then(VerdictStatus::from_str)?;
    let confidence = obj
        .get("confidence")
        .and_then(|c| c.as_f64())
        .unwrap_or(0.5);
    let rationale = obj
        .get("rationale")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let required_fixes = obj
        .get("required_fixes")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let nice_to_have = obj
        .get("nice_to_have")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Some(PhaseVerdict {
        status,
        confidence: clamp01(confidence),
        rationale,
        required_fixes,
        nice_to_have,
        duration_seconds,
        validator_name: validator.to_string(),
        stage: None,
        evidence_gate_triggered: false,
    })
}

fn parse_openai_chat(
    manifest: &Manifest,
    text: &str,
    duration: Duration,
) -> Result<ValidatorResult, OutputError> {
    let json: Value = serde_json::from_str(text).map_err(|e| OutputError::NotJson(e.to_string()))?;
    let dot_path = manifest
        .output
        .verdict_field
        .as_deref()
        .unwrap_or("choices.0.message.content");
    let content = extract_dot_path(&json, dot_path)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| OutputError::VerdictFieldMissing(dot_path.to_string()))?;
    // Recursively run the verdict-block parser on the extracted content.
    Ok(parse_verdict_block(
        manifest.adapter.name.as_str(),
        &content,
        duration,
    ))
}

fn parse_plain(validator: &str, text: &str, duration: Duration) -> ValidatorResult {
    heuristic_result(validator, text, duration)
}

fn heuristic_result(validator: &str, text: &str, duration: Duration) -> ValidatorResult {
    let lower = text.to_ascii_lowercase();
    let (status, confidence, rationale) = if lower.contains("fail") && !lower.contains("no fail") {
        (VerdictStatus::Fail, 0.5, truncate(text, 500))
    } else if lower.contains("needs fix") || lower.contains("needs_fix") {
        (VerdictStatus::NeedsFix, 0.5, truncate(text, 500))
    } else if lower.contains("pass") {
        (VerdictStatus::Pass, 0.5, truncate(text, 500))
    } else {
        (VerdictStatus::Pass, 0.5, truncate(text, 500))
    };
    wrap(
        validator,
        PhaseVerdict {
            status,
            confidence,
            rationale,
            required_fixes: Vec::new(),
            nice_to_have: Vec::new(),
            duration_seconds: duration.as_secs_f64(),
            validator_name: validator.to_string(),
            stage: None,
            evidence_gate_triggered: false,
        },
        text,
    )
}

fn wrap(validator: &str, verdict: PhaseVerdict, raw: &str) -> ValidatorResult {
    ValidatorResult {
        validator_name: validator.to_string(),
        verdict,
        raw_response: raw.to_string(),
        error: String::new(),
        flag_error_hint: String::new(),
    }
}

fn clamp01(v: f64) -> f64 {
    if v.is_nan() {
        0.5
    } else if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

fn truncate(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        s.to_string()
    } else {
        let mut end = limit;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Walk a `foo.0.bar` dot path through a JSON value. Array indices are
/// integer segments, object keys are anything else.
fn extract_dot_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(arr) => {
                let idx: usize = seg.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

// ─────────────────────────── Tests ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::manifest::{OutputFormat, OutputTable};
    use std::time::Duration;

    fn make_manifest(format: OutputFormat, verdict_field: Option<&str>) -> Manifest {
        let body = r#"
[adapter]
name = "testadapter"
version = "0.1.0"
manifest_schema = 1
description = "t"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:9/v1"

[auth]
scheme = "none"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "local"

[security]
requires_network = false
sandbox_profile = "network-io"
"#;
        let mut m = Manifest::parse_str(body).unwrap();
        m.output = OutputTable {
            format,
            parser: None,
            verdict_field: verdict_field.map(|s| s.to_string()),
        };
        m
    }

    #[test]
    fn verdict_block_json_happy_path() {
        let text = r#"
Here's my review…

---VERDICT---
{"status": "PASS", "confidence": 0.9, "rationale": "looks good"}
---END---
"#;
        let m = make_manifest(OutputFormat::LopeVerdictBlock, None);
        let r = parse_response(&m, text.as_bytes(), Duration::from_secs(1)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::Pass);
        assert_eq!(r.verdict.confidence, 0.9);
        assert_eq!(r.verdict.rationale, "looks good");
    }

    #[test]
    fn verdict_block_yaml_fallback() {
        let text = r#"
---VERDICT---
status: NEEDS_FIX
confidence: 0.8
rationale: missing retries
---END---
"#;
        let m = make_manifest(OutputFormat::LopeVerdictBlock, None);
        let r = parse_response(&m, text.as_bytes(), Duration::from_secs(1)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::NeedsFix);
        assert!((r.verdict.confidence - 0.8).abs() < 1e-9);
        assert!(r.verdict.rationale.contains("retries"));
    }

    #[test]
    fn no_verdict_block_falls_through_to_heuristic() {
        let text = "just some prose that says pass somewhere";
        let m = make_manifest(OutputFormat::LopeVerdictBlock, None);
        let r = parse_response(&m, text.as_bytes(), Duration::from_secs(1)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::Pass);
        assert!((r.verdict.confidence - 0.5).abs() < 1e-9);
    }

    #[test]
    fn openai_chat_extracts_content_and_parses_verdict() {
        let response = r#"{
            "choices": [
                {"message": {"content": "blah\n---VERDICT---\n{\"status\":\"PASS\",\"confidence\":0.95,\"rationale\":\"ok\"}\n---END---\n"}}
            ]
        }"#;
        let m = make_manifest(
            OutputFormat::OpenAiChat,
            Some("choices.0.message.content"),
        );
        let r = parse_response(&m, response.as_bytes(), Duration::from_secs(2)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::Pass);
        assert!((r.verdict.confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn openai_chat_missing_field_errors() {
        let response = r#"{"choices": []}"#;
        let m = make_manifest(
            OutputFormat::OpenAiChat,
            Some("choices.0.message.content"),
        );
        let err = parse_response(&m, response.as_bytes(), Duration::from_secs(0)).unwrap_err();
        assert!(matches!(err, OutputError::VerdictFieldMissing(_)));
    }

    #[test]
    fn openai_chat_bad_json_errors() {
        let m = make_manifest(
            OutputFormat::OpenAiChat,
            Some("choices.0.message.content"),
        );
        let err = parse_response(&m, b"not json", Duration::from_secs(0)).unwrap_err();
        assert!(matches!(err, OutputError::NotJson(_)));
    }

    #[test]
    fn openai_chat_without_verdict_block_synthesizes_heuristic_pass() {
        let response = r#"{
            "choices": [
                {"message": {"content": "Great, everything looks pass-worthy."}}
            ]
        }"#;
        let m = make_manifest(
            OutputFormat::OpenAiChat,
            Some("choices.0.message.content"),
        );
        let r = parse_response(&m, response.as_bytes(), Duration::from_secs(1)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::Pass);
        assert!((r.verdict.confidence - 0.5).abs() < 1e-9);
    }

    #[test]
    fn plain_heuristic_detects_fail() {
        let m = make_manifest(OutputFormat::Plain, None);
        let r = parse_response(&m, b"this will fail catastrophically", Duration::from_secs(1))
            .unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn plain_heuristic_detects_needs_fix() {
        let m = make_manifest(OutputFormat::Plain, None);
        let r = parse_response(&m, b"needs fix ASAP", Duration::from_secs(1)).unwrap();
        assert_eq!(r.verdict.status, VerdictStatus::NeedsFix);
    }

    #[test]
    fn custom_format_errors() {
        let m = make_manifest(OutputFormat::Custom, None);
        let err = parse_response(&m, b"anything", Duration::from_secs(0)).unwrap_err();
        assert!(matches!(err, OutputError::CustomUnsupported));
    }

    #[test]
    fn extract_dot_path_various_shapes() {
        let v: Value = serde_json::from_str(r#"{"a": [{"b": "hit"}]}"#).unwrap();
        assert_eq!(
            extract_dot_path(&v, "a.0.b").unwrap().as_str(),
            Some("hit")
        );
        assert!(extract_dot_path(&v, "a.99.b").is_none());
        assert!(extract_dot_path(&v, "nope").is_none());
    }

    #[test]
    fn truncate_long_rationale() {
        let text = "x".repeat(2000);
        let m = make_manifest(OutputFormat::Plain, None);
        let r = parse_response(&m, text.as_bytes(), Duration::from_secs(1)).unwrap();
        assert!(r.verdict.rationale.ends_with('…'));
        assert!(r.verdict.rationale.chars().count() < 600);
    }
}
