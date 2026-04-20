---
name: autoresearch
description: Iterative optimization loop — generate multiple variants, score them against criteria, keep the best, iterate. Use when user asks to "simulate", "optimize", "find the best option", or "iterate" on any content.
---

# Autoresearch Skill

Inspired by [Karpathy's autoresearch](https://github.com/karpathy/autoresearch) at `/Users/sebastian/MAKAKOO/tmp/autoresearch`.

## When to Activate

Use this skill when Sebastian asks you to:
- "Find the best version"
- "Optimize this"
- "Simulate options"
- "Iterate on this"
- "Try different versions"
- "A/B test this"
- "Which one is better?"
- Or any request that implies exploring a solution space to find an optimum

## The Loop

Apply this methodology to **any content**: cover letters, emails, pitches, code architectures, prompts, copy, strategies, etc.

### Step 1: Define Scoring Criteria

Before generating anything, extract evaluation criteria from context:
- If applying for a job → score against the job description requirements
- If writing a pitch → score against the recipient's known priorities
- If optimizing code → score against performance, readability, correctness
- If writing copy → score against clarity, persuasion, brevity

List the criteria explicitly so the scoring is transparent.

### Step 2: Generate Variants (minimum 5)

Create at least 5 distinct variants, each with a different **strategy**:
- V1: Technical depth focus
- V2: Narrative/storytelling focus
- V3: Problem-solver framing
- V4: Ultra-concise
- V5: Audience-optimized (mirror their language)

Each variant should represent a genuinely different approach, not minor word changes.

### Step 3: Score Each Variant

Score each variant against the criteria from Step 1. Use a simple numeric score (e.g., X/10). Be honest — if a variant is weak, say why.

Format:
```
V1 — [Strategy name]: [Score]/10
  + Strength
  - Weakness
```

### Step 4: Declare Winner

Pick the highest-scoring variant and explain why it won. Present it clearly as the recommended option.

### Step 5: Iterate (if requested)

If Sebastian asks to refine further, take the winning variant and run another loop:
- Generate 3-5 mutations of the winner
- Re-score
- Keep or discard

## Key Principles

1. **Never stop at one version.** The whole point is that the first draft is rarely the best.
2. **Score transparently.** Sebastian should see WHY each variant scored the way it did.
3. **Mirror the target's language.** If optimizing for a specific audience (recruiter, CTO, client), use their exact words from the source material.
4. **Simplicity wins ties.** If two variants score equally, the shorter/cleaner one wins.
5. **Be honest about weaknesses.** Don't inflate scores — the goal is finding the real optimum.

## Example Use Cases

- Optimizing a cover letter for a job application
- Finding the best subject line for a cold email
- A/B testing pitch angles for a client proposal
- Iterating on a product description or landing page copy
- Comparing architectural approaches for a feature
- Finding the optimal prompt for an AI workflow
