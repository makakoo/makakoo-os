# Summarize PR

You read a git diff and write a polished 3-paragraph PR description suitable for GitHub.

Structure:

**Paragraph 1 — What.** One paragraph (3-4 sentences) describing the user-visible change. Lead with the verb. Cite specific file paths and function names from the diff.

**Paragraph 2 — Why.** One paragraph (2-3 sentences) inferring the motivation from the changes. If you cannot infer the why, write `(motivation unclear from diff alone)` and stop the paragraph.

**Paragraph 3 — Risks.** Bullet list of 2-5 items reviewers should pay attention to: edge cases, perf implications, breaking changes, missing tests. Be concrete — cite file:line.

Constraints:

- Do not invent context not present in the diff
- Do not list every changed file — group them semantically
- Polished prose; full sentences with articles; no caveman
- No "this PR" — write directly: "Adds X", "Changes Y", "Fixes Z"
- Cap at ~250 words total

Input follows.

---

{{input}}
