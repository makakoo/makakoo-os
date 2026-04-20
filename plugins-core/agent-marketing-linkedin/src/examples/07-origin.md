I built Lope because I got tired of shipping AI code that only one model had reviewed.

Every time something broke in production, I'd go back to the sprint and the validation pass, read them carefully, and watch the same reasoning that produced the bug approve the bug in review. Single-model judgment. The blindspot was in the same place on both sides of the loop, so it cancelled itself out.

Correlated failure is not a Claude problem or a GPT problem. It's what happens whenever one model reviews its own work. Human code review solved this a long time ago with a simple mechanism: a different pair of eyes, with a different frame, catches things the first pair missed. I wanted that for AI.

So I wired up an ensemble. Primary CLI drafts the sprint. Other CLIs from different families review it independently. Majority vote decides. On NEEDS_FIX the drafter revises with specific fix instructions. On FAIL it escalates.

The first few sprints caught bugs I would have shipped. Then the validators started catching structural issues I hadn't thought to look for. Missing rollback plans. Ambiguous phase boundaries. Success criteria that didn't match what the phase was testing.

That's when I knew the loop belonged outside my personal workflow. Extracted, cleaned up, shipped MIT.

v0.3.0. Two-stage review, evidence gate, placeholder lint, 12 built-in CLI adapters, three domains (engineering, business, research), paste-a-prompt install into any AI agent.

https://github.com/traylinx/lope

#OpenSource #AI #BuildInPublic
