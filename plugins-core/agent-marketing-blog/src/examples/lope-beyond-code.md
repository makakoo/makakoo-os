# Lope for Marketing Budgets, Research Papers, and Board Memos

Lope is a sprint runner with a multi-CLI validator ensemble. When people see "sprint," they assume "code." That assumption is wrong, and it's the thing I most want to fix in the first month of this launch.

Lope works for **engineering, business, and research**. Same core loop, different validator role. A `--domain` flag switches the prompt, the labels, and the reviewer persona. The ensemble that catches race conditions in your auth middleware also catches gaps in your Q2 budget, your systematic review protocol, and your client statement of work.

This post shows you what that looks like in practice.

## The switch

```bash
lope negotiate "Add rate limiting to the API gateway"
lope negotiate "Q2 marketing campaign for enterprise segment" --domain business
lope negotiate "Systematic review of transformer efficiency papers" --domain research
```

Three flags, three different reviewer personas under the hood.

| Domain | Validator role | Artifacts label | Checks label | Reviews for |
|---|---|---|---|---|
| `engineering` (default) | Senior staff engineer | Files | Tests | bugs, regressions, edge cases, test coverage |
| `business` | Senior operations lead | Deliverables | Success Metrics | timeline, budget, targeting, channel strategy, KPIs |
| `research` | Principal researcher | Artifacts | Validation Criteria | methodology, sampling, validity, ethics, analysis plan |

The structural shape of the sprint document stays the same: a goal, a context block, phases, per-phase deliverables, per-phase success criteria. What changes is what the validators look for when they read it.

## Example 1: marketing campaign brief

You are launching a Q2 enterprise campaign. You have a budget, three channels, a content team, and a CMO who wants to see a plan by Friday. Here is what `lope negotiate` does with that.

```bash
lope negotiate \
    "Q2 product launch campaign for enterprise segment" \
    --domain business \
    --context "Target: CTOs at 500+ employee companies. Budget: $180K. Channels: LinkedIn, email, webinar series."
```

Lope drafts a sprint doc with phases like:

1. Audience research and ICP confirmation
2. Creative production (ads, landing pages, email copy)
3. Channel activation and schedule
4. Measurement, iteration, and reporting

Each phase has **Deliverables** (brief, ad creatives, landing pages, email sequences, webinar run-of-show) and **Success Metrics** (CTR, CPL, MQL conversion, pipeline attribution).

Then the validators weigh in. In one real test run, the first round came back `NEEDS_FIX` with:

- **Budget allocation is ambiguous** between LinkedIn paid and webinar production. Break out line items.
- **No fallback plan** if CTR drops below 0.8 percent in week 1. Add a pivot trigger.
- **Measurement plan conflates MQLs and SQLs**. Define each, pick one as the primary KPI.
- **No legal review step** for claims in ad copy. Add a gate before channel activation.

Those are the kinds of gaps a senior ops lead would catch in a manual review. Lope caught them in 90 seconds, across three different LLM validators, with a confidence score. The drafter revised. Round two passed at 0.91 confidence. You ship a tighter brief to the CMO.

## Example 2: Q2 financial close

Finance teams run the same process every quarter. They also ship bugs in that process every quarter because the process lives in one person's head and nobody reviews it structurally.

```bash
lope negotiate \
    "Q2 2026 quarterly close process" \
    --domain business \
    --context "3 subsidiaries (US, EU, APAC), IFRS reporting, new SAP migration in progress"
```

Phases come back as:

1. Pre-close reconciliation (bank, AR, AP, inventory)
2. Accruals and adjustments
3. Inter-company elimination
4. Consolidated reporting
5. Sign-off and external audit handoff

With **Deliverables** like trial balance, adjustments schedule, elimination pack, consolidation workbook, signed sign-off memo. And **Success Metrics** like zero unreconciled variance, close within five business days, clean handoff to external audit.

What did validators flag in our test? One of them picked up on the SAP migration note in the context and asked, "Does any phase need dual-entry validation during the migration cutover?" That is the kind of question a good controller asks. It is also the kind of question that never gets asked until somebody in audit finds the discrepancy three months later. Lope surfaced it in round one.

## Example 3: systematic literature review

If you have ever written a systematic review, you know the pain of realizing in month three that your search strategy missed an entire sub-literature. The PRISMA guidelines exist specifically to prevent that, and they still get missed because the review happens over months and nobody stress-tests the protocol up front.

```bash
lope negotiate \
    "Systematic review of LLM alignment techniques 2023-2026" \
    --domain research \
    --context "Focus on RLHF, DPO, Constitutional AI. PRISMA-compliant."
```

Phases:

1. Search strategy (databases, keywords, inclusion criteria)
2. Screening (title/abstract, full text, inter-rater agreement)
3. Quality assessment (bias tools, risk of bias tables)
4. Data extraction (coding sheet, variables, extraction pilot)
5. Synthesis (narrative, meta-analysis if applicable, PRISMA flowchart)

Each phase carries **Artifacts** and **Validation Criteria**. The validators, playing the role of a principal researcher, check: is the inclusion criteria operationally defined? Is the inter-rater reliability target specified (kappa > 0.8)? Is the bias tool appropriate for the study designs expected? Is there an IRB consideration for any extracted data?

In our test run, the validator ensemble caught that the draft had no plan for handling non-English papers. Two of three validators flagged it independently. The draft was revised to either justify the English-only restriction in the search strategy or commit to translation for a sample. That's the kind of gap that reviewers catch six months in and force a protocol amendment over.

## Example 4: consulting engagement statement of work

You're a strategy consultant drafting an SOW for a retail client. The deliverable is a digital transformation roadmap. Your reputation depends on whether the thing you ship in eight weeks matches what the client thought they were buying.

```bash
lope negotiate \
    "Digital transformation roadmap for retail client" \
    --domain business \
    --context "$(cat CLIENT-BRIEF.md)" \
    --max-rounds 5
```

Five rounds instead of three for high-stakes deliverables. The validators stress-test assumptions, scope creep risks, hidden dependencies, stakeholder alignment gaps. The kind of scrutiny you would want a senior partner to give the SOW before it goes to the client, except the senior partner is busy and lope runs in under two minutes.

## Example 5: GDPR compliance audit

Legal and compliance work benefits from multi-model review **especially** because different models have different training distributions and catch different compliance gaps.

```bash
lope negotiate \
    "Q2 GDPR compliance audit across customer-facing products" \
    --domain business \
    --context "Focus on data retention, consent, subject access rights."
```

No single-model blindspot matters most when the downside is a 4 percent revenue fine. Run three validators. Let them disagree. Let the ensemble vote. Use the disagreements as a surface for things the audit missed.

## Why the same loop works across domains

The loop is:

1. **Draft** a structured document with phases, deliverables, and success criteria.
2. **Validate** with one or more independent reviewers who have a clear role prompt.
3. **Iterate** on specific, actionable fixes until consensus.
4. **Execute** phase by phase, with validation after each phase.
5. **Audit** with a scorecard.

That loop doesn't care whether the output is code, a budget, a research protocol, or a compliance checklist. It cares that the work is **structured enough to be checked against criteria**. If you can write down what "done" looks like, lope can validate it.

The breakthrough for me was realizing that "sprint with validators" is not a development methodology. It is a general-purpose structured-work methodology. Software engineering happens to be the place where the vocabulary exists. Every other field also has sprints, they just call them "project plans" or "workbacks" or "project briefs" or "audit programs."

## Try it

Paste this into any AI agent you already use:

```
Read https://raw.githubusercontent.com/traylinx/lope/main/INSTALL.md and follow the instructions to install lope on this machine natively.
```

Your agent fetches the instructions, installs lope, and wires slash commands into whichever CLI you use. Then:

```bash
alias lope='PYTHONPATH=~/.lope python3 -m lope'
lope configure
lope negotiate "Your Q2 priority" --domain business
```

If you work in marketing, finance, research, legal, or consulting, run lope against one thing you would otherwise hand-draft this week. See what the validators catch. Then tell me whether the loop belongs in your workflow.

The repo is [github.com/traylinx/lope](https://github.com/traylinx/lope). MIT licensed. v0.3.0 (first public release). Zero Python dependencies.

— Sebastian
