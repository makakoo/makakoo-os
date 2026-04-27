# Skills to Improve

**Priority Queue** — Higher priority = improve first
**Format:** `| skill-path | priority | status | last-improved | notes |`

---

## Priority 1: Critical (Known Gaps)

| Skill Path | Priority | Status | Last Improved | Notes |
|------------|----------|--------|---------------|-------|
| `dev/writing-skills` | P1 | done | 2026-03-29 | Core skill - TDD for docs. Missing rationalization counters |
| `dev/plan` | P1 | done | 2026-03-29 | Simple but may need CSO improvement |
| `career/career-manager` | P1 | in-progress | never | High-value, complex SOP - needs pressure testing |

---

## Priority 2: High Frequency (Used Often)

| Skill Path | Priority | Status | Last Improved | Notes |
|------------|----------|--------|---------------|-------|
| `dev/autoplan` | P2 | done | 2026-03-29 | Very frequently used |
| `dev/plan-ceo-review` | P2 | done | 2026-03-29 | Strategy workhorse |
| `dev/investigate` | P2 | in-progress | never | Debugging core |
| `dev/review` | P2 | pending | never | Code review高频 |
| `browser-qa/browse` | P2 | pending | never | Daily driver |
| `meta/cso` | P2 | pending | never | Security gate |
| `blockchain/polymarket` | P2 | pending | never | Prediction market data — trading foundation |
| `arbitrage-agent` | P2 | pending | never | Paper trading bot — needs cron + skill docs |

---

## Priority 3: Medium (Could Be Better)

| Skill Path | Priority | Status | Last Improved | Notes |
|------------|----------|--------|---------------|-------|
| `dev/qa` | P3 | pending | never | QA skill |
| `dev/qa-only` | P3 | pending | never | QA report only |
| `dev/design-review` | P3 | pending | never | Visual QA |
| `browser-qa/canary` | P3 | pending | never | Deploy monitoring |
| `research/duckduckgo-search` | P3 | pending | never | Research tool |

---

## Priority 4: Low (Nice to Have)

| Skill Path | Priority | Status | Last Improved | Notes |
|------------|----------|--------|---------------|-------|
| `ai-ml/*` | P4 | pending | never | Many ML skills, review later |
| `lifestyle/*` | P4 | pending | never | Lower impact for career goals |

---

## Frozen (Do Not Modify)

These skills are explicitly locked and should not be modified by the autoimprover:

| Skill Path | Reason |
|------------|--------|
| `meta/gstack-upgrade` | Requires specific version checks |
| `dev/ship` | Critical deploy pipeline |
| `dev/land-and-deploy` | Production safety |

---

## Adding New Skills

Add to the appropriate priority tier when:
- New skill is created
- Skill is flagged as problematic
- Sebastian requests specific improvement

Format: `| path | P# | pending | never | description of issue |`

---

## Batch Operations

To move all skills in a tier to "pending":
```bash
# Example: Reset all P3 to pending
sed -i '' 's/P3.*status.*[a-z]/P3 | pending | never |/g' skills_to_improve.md
```
