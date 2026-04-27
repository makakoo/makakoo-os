Shipped: Lope. Open source, MIT, v0.3.0.

One AI CLI writes the code. A different AI CLI reviews it. Majority vote decides if the phase ships. No single-model blindspot.

12 built-in validators out of the box: Claude Code, OpenCode, Gemini CLI, OpenAI Codex, Mistral Vibe, Aider, Ollama, Goose, Open Interpreter, llama.cpp, GitHub Copilot CLI, Amazon Q. Add any other AI backend via 5 lines of JSON config. Zero Python dependencies. Pure stdlib. No pip, no venv, no broken wheels.

Lope is an autonomous sprint runner. It drafts a structured plan, negotiates it with your validator ensemble, executes phase by phase with validator-in-the-loop retry, and produces a scorecard. Works for engineering, business (marketing, finance, ops, consulting), and research. Same loop, different validator role.

What's new in v0.3.0: two-stage validator review (spec compliance then code quality), verification-before-completion gate that auto-downgrades rubber-stamp PASSes, no-placeholder lint on drafts, SessionStart hook so agents know lope exists, and a "using-lope" auto-trigger skill so users don't have to type slash commands. Describe multi-phase work in natural language and the agent invokes lope for you.

Install is one line you paste into any AI agent:

Read https://raw.githubusercontent.com/traylinx/lope/main/INSTALL.md and follow the instructions to install lope on this machine natively.

Your agent fetches the instructions, installs lope, wires slash commands into Claude Code / Codex / Gemini CLI / OpenCode / Cursor, and reports back when it's live.

Repo: https://github.com/traylinx/lope

Star it, break it, open issues.

#AIEngineering #OpenSource #DeveloperTools #LLM #CodingAssistants
