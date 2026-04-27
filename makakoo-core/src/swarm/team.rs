//! Team compositions — pre-wired rosters the SwarmGateway can dispatch as a unit.
//!
//! Port of `plugins-core/lib-harvey-core/src/core/orchestration/agent_team.py`.
//! Keeps the Rust path feature-flagged behind `kernel.swarm_v2`; the Python
//! side remains the default until Phase A gate signs off.
//!
//! Design:
//!   * `TeamMember` = one role (agent + action + optional parallelism).
//!   * `TeamRoster` = immutable recipe: members + total step count +
//!     default parallelism knob.
//!   * `TeamComposition` enum factories: `research`, `archive`, `minimal`.
//!     `creative` stays in Python (image-gen plugin) until an image-gen
//!     handler ships in Rust.
//!
//! Build the roster, then feed each member into `SwarmGateway::dispatch`
//! in dependency order. Roles with `depends_on` must block on their
//! parent's artifacts before running.

use serde::{Deserialize, Serialize};

/// One role on a team. `count > 1` means spawn N parallel instances of
/// the same (agent, action) pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMember {
    /// Subagent name — must match a name the coordinator knows how to spawn.
    pub agent: String,
    /// One of that agent's actions (e.g. "search_all", "combine", "save_to_brain").
    pub action: String,
    /// Human-readable role label (e.g. "parallel_researcher", "synthesizer").
    pub role: String,
    /// Parallelism for this role.
    pub count: usize,
    /// Other roles whose artifacts must be available before this role runs.
    pub depends_on_roles: Vec<String>,
}

impl TeamMember {
    pub fn new(agent: impl Into<String>, action: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            action: action.into(),
            role: role.into(),
            count: 1,
            depends_on_roles: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_count(mut self, count: usize) -> Self {
        self.count = count.max(1);
        self
    }

    #[must_use]
    pub fn depends_on(mut self, roles: &[&str]) -> Self {
        self.depends_on_roles = roles.iter().map(|r| r.to_string()).collect();
        self
    }
}

/// Compile-time team definition. Immutable once constructed — the gateway
/// walks this structure to drive coordinator dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamRoster {
    pub name: String,
    pub description: String,
    pub members: Vec<TeamMember>,
    /// Only meaningful for research_team's scaling knob; other rosters
    /// ignore this field.
    pub default_parallelism: usize,
}

impl TeamRoster {
    pub fn role_ids(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|m| {
                if m.role.is_empty() {
                    m.agent.clone()
                } else {
                    m.role.clone()
                }
            })
            .collect()
    }

    pub fn total_steps(&self) -> usize {
        self.members.iter().map(|m| m.count).sum()
    }
}

/// Factory over the built-in rosters. Mirrors the Python `TeamComposition`
/// class — extend with new constructors as more subagent flavors land.
pub struct TeamComposition;

impl TeamComposition {
    /// N parallel researchers → synthesizer → storage. Canonical fan-out
    /// pattern; bump `parallelism` to scale research horizontally.
    pub fn research_team(parallelism: usize) -> TeamRoster {
        let parallelism = parallelism.max(1);
        TeamRoster {
            name: "research_team".into(),
            description: format!("{parallelism}× researcher → synthesizer → storage"),
            members: vec![
                TeamMember::new("researcher", "search_all", "parallel_researcher")
                    .with_count(parallelism),
                TeamMember::new("synthesizer", "combine", "synthesizer")
                    .depends_on(&["parallel_researcher"]),
                TeamMember::new("storage", "save_to_brain", "storage")
                    .depends_on(&["synthesizer"]),
            ],
            default_parallelism: parallelism,
        }
    }

    /// Single researcher → storage. "Just save this" fast path — skips
    /// synthesis.
    pub fn archive_team() -> TeamRoster {
        TeamRoster {
            name: "archive_team".into(),
            description: "researcher → storage (no synthesis)".into(),
            members: vec![
                TeamMember::new("researcher", "search_all", "researcher"),
                TeamMember::new("storage", "save_to_brain", "storage")
                    .depends_on(&["researcher"]),
            ],
            default_parallelism: 1,
        }
    }

    /// Single researcher, nothing downstream. Smoke-test roster.
    pub fn minimal_team() -> TeamRoster {
        TeamRoster {
            name: "minimal_team".into(),
            description: "single researcher only".into(),
            members: vec![TeamMember::new("researcher", "search_all", "researcher")],
            default_parallelism: 1,
        }
    }

    /// Look up a roster by name. Returns `None` for unknown names so the
    /// caller can emit a clean error rather than panicking.
    pub fn by_name(name: &str, parallelism: Option<usize>) -> Option<TeamRoster> {
        match name {
            "research_team" => Some(Self::research_team(parallelism.unwrap_or(2))),
            "archive_team" => Some(Self::archive_team()),
            "minimal_team" => Some(Self::minimal_team()),
            // creative_team intentionally unimplemented — routed to Python
            // image-gen plugin until a Rust image-gen handler exists.
            _ => None,
        }
    }

    /// Names of every team the Rust side can dispatch natively.
    pub fn available_names() -> &'static [&'static str] {
        &["research_team", "archive_team", "minimal_team"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_team_has_three_roles_and_scales_parallelism() {
        let r = TeamComposition::research_team(4);
        assert_eq!(r.name, "research_team");
        assert_eq!(r.members.len(), 3);
        assert_eq!(r.default_parallelism, 4);
        assert_eq!(r.total_steps(), 4 + 1 + 1);
        assert_eq!(r.members[0].count, 4);
        assert_eq!(r.members[1].depends_on_roles, vec!["parallel_researcher"]);
        assert_eq!(r.members[2].depends_on_roles, vec!["synthesizer"]);
    }

    #[test]
    fn research_team_floors_parallelism_at_one() {
        let r = TeamComposition::research_team(0);
        assert_eq!(r.default_parallelism, 1);
        assert_eq!(r.members[0].count, 1);
    }

    #[test]
    fn archive_team_has_two_sequential_roles() {
        let r = TeamComposition::archive_team();
        assert_eq!(r.members.len(), 2);
        assert_eq!(r.members[0].role, "researcher");
        assert_eq!(r.members[1].depends_on_roles, vec!["researcher"]);
        assert_eq!(r.total_steps(), 2);
    }

    #[test]
    fn minimal_team_has_single_member() {
        let r = TeamComposition::minimal_team();
        assert_eq!(r.members.len(), 1);
        assert_eq!(r.total_steps(), 1);
        assert!(r.members[0].depends_on_roles.is_empty());
    }

    #[test]
    fn by_name_returns_known_rosters_and_none_for_unknown() {
        assert!(TeamComposition::by_name("research_team", None).is_some());
        assert!(TeamComposition::by_name("archive_team", None).is_some());
        assert!(TeamComposition::by_name("minimal_team", None).is_some());
        assert!(TeamComposition::by_name("creative_team", None).is_none());
        assert!(TeamComposition::by_name("nonsense", None).is_none());
    }

    #[test]
    fn role_ids_are_unique_and_in_composition_order() {
        let r = TeamComposition::research_team(3);
        assert_eq!(
            r.role_ids(),
            vec!["parallel_researcher", "synthesizer", "storage"]
        );
    }

    #[test]
    fn available_names_matches_by_name_acceptance() {
        for name in TeamComposition::available_names() {
            assert!(
                TeamComposition::by_name(name, None).is_some(),
                "{name} advertised but by_name rejects it"
            );
        }
    }
}
