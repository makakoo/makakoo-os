//! Session tree exporters — Markdown + HTML + JSON.
//!
//! v0.2 Phase G.4. Reads a loaded entry stream and renders it for
//! human (or downstream-tool) consumption. Pure functions — no I/O,
//! no filesystem, no network. The CLI wraps these with file writers.

use super::tree::{Entry, MessageRole};

/// Format an entry timestamp as a compact `YYYY-MM-DD HH:MM:SS` stamp.
fn ts(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

/// Render the session as Markdown. Each entry becomes one block with
/// a role heading + body; non-message entries (fork, label, rewind,
/// compaction) collapse to terse one-line markers so the document
/// stays scannable even for long sessions.
pub fn to_markdown(entries: &[Entry]) -> String {
    let mut out = String::new();
    for e in entries {
        match e {
            Entry::Session { id, created_at, parent_session, fork_from_entry, title, .. } => {
                out.push_str(&format!("# Session `{}`\n\n", id));
                out.push_str(&format!("- created: {}\n", ts(created_at)));
                if let Some(t) = title {
                    out.push_str(&format!("- title: {}\n", t));
                }
                if let Some(p) = parent_session {
                    out.push_str(&format!("- forked-from: session `{}`\n", p));
                }
                if let Some(p) = fork_from_entry {
                    out.push_str(&format!("- fork-point: entry `{}`\n", p));
                }
                out.push('\n');
            }
            Entry::Message { role, content, ts: t, .. } => {
                out.push_str(&format!("### {} — {}\n\n", role_label(role), ts(t)));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Entry::Compaction { first_kept_entry_id, tokens_before, tokens_after, ts: t, .. } => {
                out.push_str(&format!(
                    "> _compaction @ {}_ — {}→{} tokens, first-kept `{}`\n\n",
                    ts(t), tokens_before, tokens_after, first_kept_entry_id,
                ));
            }
            Entry::BranchSummary { branch_session_id, summary, ts: t, .. } => {
                out.push_str(&format!(
                    "> _branch @ {}_ → session `{}`: {}\n\n",
                    ts(t), branch_session_id, summary,
                ));
            }
            Entry::Custom { kind, payload, ts: t, .. } => {
                out.push_str(&format!("> _{}_ @ {}\n\n", kind, ts(t)));
                out.push_str("```json\n");
                out.push_str(&serde_json::to_string_pretty(payload).unwrap_or_default());
                out.push_str("\n```\n\n");
            }
            Entry::CustomMessage { kind, role, content, ts: t, .. } => {
                out.push_str(&format!("### {} ({}) — {}\n\n", role_label(role), kind, ts(t)));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Entry::Label { name, ts: t, .. } => {
                out.push_str(&format!("> 🏷️ _label_ `{}` @ {}\n\n", name, ts(t)));
            }
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the session as a single self-contained HTML document — no
/// external CSS, no JS, no fonts fetched from the network. Safe to
/// mail, attach, or host on a static bucket.
pub fn to_html(entries: &[Entry]) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n<title>Makakoo session export</title>\n");
    out.push_str("<style>\n");
    out.push_str("body{font:14px/1.5 -apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif;max-width:780px;margin:2em auto;padding:0 1em;color:#1a1a1a}\n");
    out.push_str("h1{border-bottom:1px solid #ddd;padding-bottom:.3em}\n");
    out.push_str(".turn{margin:1.2em 0;padding:.8em 1em;border-radius:8px}\n");
    out.push_str(".turn.user{background:#f0f4ff}\n");
    out.push_str(".turn.assistant{background:#f7f7f7}\n");
    out.push_str(".turn.system{background:#fff8e1}\n");
    out.push_str(".turn.tool{background:#eef8ee;font-family:ui-monospace,SFMono-Regular,monospace}\n");
    out.push_str(".meta{color:#666;font-size:12px;margin-bottom:.4em}\n");
    out.push_str(".marker{color:#888;font-style:italic;margin:.6em 0}\n");
    out.push_str(".label{color:#9b59b6}\n");
    out.push_str("pre{background:#272822;color:#f8f8f2;padding:.6em;border-radius:6px;overflow:auto}\n");
    out.push_str("</style>\n</head>\n<body>\n");

    for e in entries {
        match e {
            Entry::Session { id, created_at, title, parent_session, .. } => {
                out.push_str(&format!(
                    "<h1>Session <code>{}</code></h1>\n",
                    html_escape(id),
                ));
                out.push_str(&format!(
                    "<div class=\"meta\">created {}{}{}</div>\n",
                    ts(created_at),
                    title.as_deref()
                        .map(|t| format!(" · title: {}", html_escape(t)))
                        .unwrap_or_default(),
                    parent_session.as_deref()
                        .map(|p| format!(" · forked from <code>{}</code>", html_escape(p)))
                        .unwrap_or_default(),
                ));
            }
            Entry::Message { role, content, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"turn {role}\"><div class=\"meta\">{role} · {t_ts}</div><div>{body}</div></div>\n",
                    role = role_label(role),
                    t_ts = ts(t),
                    body = html_escape(content).replace('\n', "<br>"),
                ));
            }
            Entry::CustomMessage { kind, role, content, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"turn {role}\"><div class=\"meta\">{role} ({kind}) · {t_ts}</div><div>{body}</div></div>\n",
                    role = role_label(role),
                    kind = html_escape(kind),
                    t_ts = ts(t),
                    body = html_escape(content).replace('\n', "<br>"),
                ));
            }
            Entry::Compaction { tokens_before, tokens_after, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"marker\">compaction at {} — {}→{} tokens</div>\n",
                    ts(t), tokens_before, tokens_after,
                ));
            }
            Entry::BranchSummary { branch_session_id, summary, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"marker\">branch at {} → <code>{}</code>: {}</div>\n",
                    ts(t), html_escape(branch_session_id), html_escape(summary),
                ));
            }
            Entry::Label { name, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"marker label\">🏷️ label <strong>{}</strong> at {}</div>\n",
                    html_escape(name), ts(t),
                ));
            }
            Entry::Custom { kind, payload, ts: t, .. } => {
                out.push_str(&format!(
                    "<div class=\"marker\">{kind} at {t_ts}</div>\n<pre>{body}</pre>\n",
                    kind = html_escape(kind),
                    t_ts = ts(t),
                    body = html_escape(&serde_json::to_string_pretty(payload).unwrap_or_default()),
                ));
            }
        }
    }

    out.push_str("</body>\n</html>\n");
    out
}

/// Render the session as a single pretty JSON document — every entry
/// in one `entries` array, alongside a `session` header block. Easier
/// to slurp into other tools than hand-parsing the JSONL.
pub fn to_json(entries: &[Entry]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "version": 1,
        "entries": entries,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::tree::{Entry, MessageRole};
    use super::*;
    use chrono::Utc;

    fn sample() -> Vec<Entry> {
        let now = Utc::now();
        vec![
            Entry::Session {
                id: "s1".into(),
                created_at: now,
                parent_session: None,
                fork_from_entry: None,
                title: Some("test".into()),
            },
            Entry::Message {
                id: "m1".into(),
                parent_id: Some("s1".into()),
                role: MessageRole::User,
                content: "hi".into(),
                ts: now,
            },
            Entry::Message {
                id: "m2".into(),
                parent_id: Some("m1".into()),
                role: MessageRole::Assistant,
                content: "hello\nworld".into(),
                ts: now,
            },
            Entry::Label {
                id: "l1".into(),
                parent_id: "m2".into(),
                name: "checkpoint".into(),
                ts: now,
            },
        ]
    }

    #[test]
    fn markdown_contains_header_and_roles() {
        let md = to_markdown(&sample());
        assert!(md.contains("# Session `s1`"));
        assert!(md.contains("### user"));
        assert!(md.contains("### assistant"));
        assert!(md.contains("🏷️"));
        assert!(md.contains("checkpoint"));
    }

    #[test]
    fn html_escapes_content() {
        let mut entries = sample();
        if let Entry::Message { content, .. } = &mut entries[1] {
            *content = "<script>alert(1)</script>".into();
        }
        let html = to_html(&entries);
        assert!(!html.contains("<script>alert(1)"), "raw script leaked");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<!doctype html>"));
    }

    #[test]
    fn html_preserves_newlines_as_br() {
        let html = to_html(&sample());
        assert!(html.contains("hello<br>world"));
    }

    #[test]
    fn json_is_valid_and_roundtrips() {
        let json = to_json(&sample());
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 4);
    }
}
