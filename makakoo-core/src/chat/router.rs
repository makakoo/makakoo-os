//! `IntelligentRouter` — keyword-heuristic message triage.
//!
//! Ports the classification heuristics from
//! `harvey-os/core/orchestration/intelligent_router.py` into a simpler
//! three-way decision: direct LLM answer, tool call, or slash command. The
//! Python source also returns `TeamRoster`s; that layer lives above the
//! chat store and is outside this task's scope.
//!
//! Decision rules (deterministic for stable tests):
//!
//! 1. If the message starts with `/`, it is a [`RouteDecision::Command`]
//!    containing the whitespace-tokenised argv, with the leading `/`
//!    stripped from the first token.
//! 2. Otherwise, match the (lower-cased) message against each intent
//!    keyword bucket. If any bucket has a hit, return
//!    [`RouteDecision::ToolCall`] with the bucket's canonical tool name.
//!    Ties are broken by `PRIORITY_ORDER`.
//! 3. Otherwise, return [`RouteDecision::DirectAnswer`].
//!
//! Phase 4 can swap `route()` for an LLM call without touching callers.

use std::collections::HashMap;

/// What the router thinks the user wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    /// Call the LLM directly with the existing chat history as context.
    DirectAnswer,
    /// Dispatch to a named tool (canonical `brain_search`, `image_generate`,
    /// `brain_write`, `minimal` …).
    ToolCall(String),
    /// Parsed slash command argv. The leading `/` has been stripped.
    Command(Vec<String>),
}

/// Deterministic keyword classifier.
#[derive(Debug, Clone, Default)]
pub struct IntelligentRouter;

/// Intents the router can pick and the tool each maps to.
const INTENT_TOOLS: &[(&str, &str)] = &[
    ("research", "brain_search"),
    ("image", "image_generate"),
    ("archive", "brain_write"),
    ("minimal", "minimal_answer"),
];

/// Tie-break order. Earlier entries beat later ones when hit-counts are
/// equal. Mirrors Python `PRIORITY_ORDER`.
const PRIORITY_ORDER: &[&str] = &["image", "research", "archive", "minimal"];

impl IntelligentRouter {
    pub fn new() -> Self {
        Self
    }

    /// Classify a raw user message and return a routing decision.
    pub fn route(&self, message: &str) -> RouteDecision {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return RouteDecision::DirectAnswer;
        }

        if let Some(rest) = trimmed.strip_prefix('/') {
            let mut tokens: Vec<String> =
                rest.split_whitespace().map(|s| s.to_string()).collect();
            if tokens.is_empty() {
                return RouteDecision::DirectAnswer;
            }
            // Guarantee at least the command name is present.
            tokens.truncate(64);
            return RouteDecision::Command(tokens);
        }

        let normalized = trimmed.to_lowercase();
        let mut hits: HashMap<&'static str, usize> = HashMap::new();
        for (intent, keywords) in intent_keywords() {
            let mut count = 0;
            for kw in keywords.iter() {
                if normalized.contains(*kw) {
                    count += 1;
                }
            }
            if count > 0 {
                hits.insert(*intent, count);
            }
        }
        if hits.is_empty() {
            return RouteDecision::DirectAnswer;
        }

        let max_hits = *hits.values().max().unwrap();
        for intent in PRIORITY_ORDER {
            if hits.get(intent).copied().unwrap_or(0) == max_hits {
                let tool = INTENT_TOOLS
                    .iter()
                    .find(|(i, _)| i == intent)
                    .map(|(_, t)| *t)
                    .unwrap_or("brain_search");
                return RouteDecision::ToolCall(tool.to_string());
            }
        }
        RouteDecision::DirectAnswer
    }
}

fn intent_keywords() -> &'static [(&'static str, &'static [&'static str])] {
    const RESEARCH: &[&str] = &[
        "research",
        "find",
        "search",
        "investigate",
        "literature",
        "compare",
        "sources",
        "papers",
        "study",
        "lookup",
        "explore",
        "what is",
        "who is",
        "how does",
        "analyze",
    ];
    const IMAGE: &[&str] = &[
        "image",
        "picture",
        "photo",
        "draw",
        "illustration",
        "render",
        "generate image",
        "create image",
        "logo",
        "icon",
        "artwork",
        "visualize",
        "painting",
        "sketch",
    ];
    const ARCHIVE: &[&str] = &[
        "save",
        "archive",
        "remember",
        "store",
        "persist",
        "log this",
        "record",
        "bookmark",
        "write to brain",
    ];
    const MINIMAL: &[&str] = &[
        "quick",
        "briefly",
        "tl;dr",
        "one-liner",
        "short answer",
    ];
    &[
        ("research", RESEARCH),
        ("image", IMAGE),
        ("archive", ARCHIVE),
        ("minimal", MINIMAL),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_message_falls_through_to_direct() {
        let r = IntelligentRouter::new();
        assert_eq!(r.route(""), RouteDecision::DirectAnswer);
        assert_eq!(r.route("   "), RouteDecision::DirectAnswer);
    }

    #[test]
    fn slash_command_is_parsed_without_slash() {
        let r = IntelligentRouter::new();
        let out = r.route("/help me please");
        match out {
            RouteDecision::Command(tokens) => {
                assert_eq!(tokens, vec!["help", "me", "please"]);
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn bare_slash_is_direct_answer() {
        let r = IntelligentRouter::new();
        assert_eq!(r.route("/"), RouteDecision::DirectAnswer);
    }

    #[test]
    fn research_phrase_routes_to_brain_search() {
        let r = IntelligentRouter::new();
        assert_eq!(
            r.route("please research superconductors today"),
            RouteDecision::ToolCall("brain_search".into())
        );
        assert_eq!(
            r.route("what is Polymarket"),
            RouteDecision::ToolCall("brain_search".into())
        );
    }

    #[test]
    fn image_phrase_routes_to_image_generate() {
        let r = IntelligentRouter::new();
        assert_eq!(
            r.route("generate image of a banana mascot"),
            RouteDecision::ToolCall("image_generate".into())
        );
        assert_eq!(
            r.route("draw a sketch of a parrot"),
            RouteDecision::ToolCall("image_generate".into())
        );
    }

    #[test]
    fn archive_phrase_routes_to_brain_write() {
        let r = IntelligentRouter::new();
        assert_eq!(
            r.route("remember this for later"),
            RouteDecision::ToolCall("brain_write".into())
        );
        assert_eq!(
            r.route("save and bookmark the decision"),
            RouteDecision::ToolCall("brain_write".into())
        );
    }

    #[test]
    fn unmatched_falls_to_direct_answer() {
        let r = IntelligentRouter::new();
        assert_eq!(
            r.route("what time is dinner"),
            RouteDecision::DirectAnswer
        );
    }

    #[test]
    fn image_wins_tie_over_archive() {
        // Both `image` (1) and `archive` (1) score one hit; `image`
        // outranks `archive` in PRIORITY_ORDER.
        let r = IntelligentRouter::new();
        assert_eq!(
            r.route("save an image"),
            RouteDecision::ToolCall("image_generate".into())
        );
    }
}
