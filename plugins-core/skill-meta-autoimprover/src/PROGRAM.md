# Harvey Auto-Improver: Program

**Version:** 1.0.0
**Pattern:** Autoresearch (Karpathy-style autonomous improvement)
**Goal:** Continuously improve Harvey's skill library through experimentation

---

## Overview

This is the agentic core of Harvey's self-improvement system. It follows the same pattern as Karpathy's autoresearch: give an agent a single editable file, a metric to optimize, and let it experiment autonomously.

For Harvey, the "model" is the SKILL.md files, and the "metric" is agent compliance under pressure scenarios.

---

## Setup Phase

1. **Initialize run:**
   - Read `skills_to_improve.md` for the priority queue
   - Read `evaluate_skill.py` to understand the evaluation framework
   - Verify `~/.harvey/autoimprover/results.tsv` exists (create if not)

2. **Select skill:**
   - Pick the highest-priority skill from `skills_to_improve.md`
   - If all skills are "recently improved" (improved in last 7 days), sleep and retry tomorrow
   - Mark skill as "in progress" in the queue

3. **Read context:**
   - Read the current `SKILL.md` for the target skill
   - Read any existing test scenarios in `test_scenarios/[skill-name]/`
   - Read the skill's category directory structure for context

---

## Improvement Loop (runs until skill is optimized or max iterations)

```
FOR each improvement iteration (max 5 per skill per run):

1. ANALYZE current skill:
   - Identify gaps: missing trigger phrases, unclear instructions, gaps in coverage
   - Check for rationalization loopholes (no explicit counters)
   - Evaluate CSO optimization (description, keywords, naming)

2. GENERATE test scenario:
   - Create a pressure scenario targeting one specific gap
   - Scenario should test: does an agent follow the skill under pressure?
   - Save to `test_scenarios/[skill-name]/scenario_[n].md`

3. RUN baseline test (WITHOUT skill):
   - Dispatch subagent with ONLY the scenario prompt
   - Do NOT load the SKILL.md into context
   - Measure: task completion, rule compliance, output quality
   - Record baseline_score

4. IMPROVE the skill:
   - Edit the SKILL.md to address the identified gap
   - Add: clearer trigger phrases, explicit counters, better CSO
   - Do NOT add hypothetical improvements - only fix what was tested

5. RUN improved test (WITH skill):
   - Dispatch same subagent WITH SKILL.md loaded
   - Measure same metrics
   - Record improved_score

6. EVALUATE:
   - delta = improved_score - baseline_score
   - If delta > 0 (improvement):
     - git commit with message: "improve([skill]): [brief description]"
     - Update `skills_to_improve.md` priority (lower priority, it improved)
     - Log to results.tsv
   - If delta <= 0 (no improvement or worse):
     - git reset to previous state
     - Mark this gap as "known limitation" instead
     - Log to results.tsv with no-improvement flag

7. REPEAT with next gap, or move to next skill if no more gaps
```

---

## Skill Quality Metrics (used by evaluate_skill.py)

| Metric | Score Range | What it Measures |
|--------|-------------|------------------|
| **Coverage** | 0-100 | % of trigger scenarios addressed |
| **Compliance** | 0-100 | Agent follows rules under pressure |
| **Discovery** | 0-100 | CSO optimization (keywords, description) |
| **Clarity** | 0-100 | No ambiguous instructions |
| **Completeness** | 0-100 | Edge cases, rationalization counters |

**Composite Score:** (Coverage + Compliance + Discovery + Clarity + Completeness) / 5

---

## Constraints

- **Single editable file per skill:** Only the SKILL.md can be modified
- **No new dependencies:** Cannot install packages
- **No structural changes:** Cannot move skill directories
- **Sacred metric:** The evaluation framework (`evaluate_skill.py`) is fixed
- **Never stop:** Run continuously until no more improvements possible
- **Log everything:** Every experiment goes to results.tsv

---

## Git Workflow

- No branches - improvements commit directly to main
- Commit message format: `improve([skill-name]): [brief change description]`
- Commits should be atomic (one improvement per commit)
- Tag releases: `autoimprover/v.YYYY-MM-DD` weekly

---

## Results Log Format (results.tsv)

```
date	time	skill	gap_description	baseline_score	improved_score	delta	status
2026-03-27	16:30	writing-skills	missing rationalization counter	45	72	+27	improved
2026-03-27	16:45	plan-ceo-review	weak description keywords	60	62	+2	improved
2026-03-27	17:00	qatest	no edge case coverage	55	55	0	no-improvement
```

---

## Exit Criteria

The system runs indefinitely when scheduled. Each run processes:
- Up to 3 skills per hour
- Up to 5 improvement iterations per skill per run
- Then exits cleanly for the next scheduled run

---

## Human Override

Sebastian can:
1. Edit `skills_to_improve.md` to prioritize specific skills
2. Add "frozen" tag to skills that shouldn't be modified
3. Add specific improvement instructions in the skill's queue entry
4. Kill the process at any time: `pkill -f run_improvements.py`
