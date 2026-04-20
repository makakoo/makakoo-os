#!/usr/bin/env python3
"""
Superbrain Global CLI — Available from any directory, any AI CLI.

Install: ln -sf ~/MAKAKOO/harvey-os/core/superbrain/cli.py /usr/local/bin/superbrain

Commands:
  superbrain query "question"     # Search + synthesize
  superbrain search "keywords"    # FTS5 keyword search (raw)
  superbrain sync [--force]       # Re-index Brain
  superbrain status               # Show health
  superbrain stack                # Memory stack (L0+L1)
  superbrain stack "query"        # Memory stack with L2
  superbrain gods                 # Top entities
  superbrain remember "summary"   # Log something to Brain
  superbrain context              # Compact context for injection
  superbrain index                # Rebuild Brain Index page
  superbrain lint                 # Wiki health check
  superbrain compile [DATE]       # Compile journal → wiki pages (with contradiction detection)
  superbrain compile-all [DAYS]   # Compile recent journals
  superbrain save "Title" "Text"  # File answer back into wiki as page
  superbrain buddy                # Meet your companion
  superbrain buddy stats          # Full stat card
  superbrain buddy pet            # Pet your buddy (♥)
  superbrain buddy speak          # Mood-based speech bubble
  superbrain buddy animate        # 3-frame sprite animation
  superbrain nursery              # Roll call — see all mascots
  superbrain nursery hatch        # Hatch a random mascot
  superbrain nursery show <name>  # Show mascot details
  superbrain nursery feed         # Feed all mascots
  superbrain nursery mood         # Family psych level
  superbrain dream                # Run memory consolidation
  superbrain dream --status       # Check dream gate status
  superbrain dream --force        # Force dream cycle
  superbrain setup                 # Auto-detect CLIs and install Harvey MCP in all
  superbrain setup --check         # Verify Harvey is connected to all CLIs
  superbrain agent install <url>   # Install agent from GitHub URL
  superbrain agent uninstall <n>  # Uninstall an agent
  superbrain agent create <name>  # Scaffold a new agent from scratch
  superbrain agent list           # List installed agents
  superbrain agent info <name>    # Show agent details
  superbrain sancho status        # SANCHO proactive engine status
  superbrain sancho tick          # Run one proactive tick
  superbrain swarm "objective"    # Launch multi-agent swarm
  superbrain swarm list           # List swarm tasks
  superbrain swarm status <id>    # Show swarm task details
  superbrain costs                # Session cost report
  superbrain costs --history      # Historical cost analysis
"""

import json
import os
import sys

# Ensure harvey-os is on path regardless of working directory
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))

# Load .env
env_path = os.path.join(HARVEY_HOME, ".env")
if os.path.exists(env_path):
    with open(env_path) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ.setdefault(k.strip(), v.strip())


def main():
    from core.superbrain.superbrain import Superbrain

    sb = Superbrain()

    if len(sys.argv) < 2 or sys.argv[1] in ("--help", "-h", "help"):
        print("superbrain — Harvey's global knowledge layer\n")
        print("  query \"question\"      Search + LLM synthesis")
        print("  search \"keywords\"     FTS5 keyword search (no LLM)")
        print("  sync [--force]        Re-index Brain → SQLite FTS5")
        print("  status                Show store health")
        print("  stack [\"query\"]       Show memory context")
        print("  context               Compact context (for piping)")
        print("  gods [N]              Top N entities")
        print("  neighbors \"entity\"    Entity relationships")
        print("  remember \"summary\"    Log to Brain journal + store")
        return

    cmd = sys.argv[1]

    # ── query ────────────────────────────────────────────────
    if cmd == "query":
        question = " ".join(sys.argv[2:])
        if not question:
            print("Usage: superbrain query \"your question\"")
            sys.exit(1)
        result = sb.query(question)
        print(result.answer)
        if result.sources:
            print(f"\n[{len(result.sources)} sources, {result.query_time_sec:.1f}s: {', '.join(result.systems_queried)}]")

    # ── search (raw, no LLM) ────────────────────────────────
    elif cmd == "search":
        keywords = " ".join(sys.argv[2:])
        if not keywords:
            print("Usage: superbrain search \"keywords\"")
            sys.exit(1)
        results = sb.store.search(keywords, top_k=10)
        for r in results:
            print(f"[{r['doc_type']:7}] {r['name']:<35} score={r['score']:.3f}")
            # Snippet
            content = r['content']
            words = keywords.lower().split()
            idx = -1
            for w in words:
                idx = content.lower().find(w)
                if idx >= 0:
                    break
            start = max(0, idx - 40) if idx >= 0 else 0
            print(f"         {content[start:start+160].strip()}\n")

    # ── sync ─────────────────────────────────────────────────
    elif cmd == "sync":
        force = "--force" in sys.argv
        embed = "--embed" in sys.argv
        result = sb.sync(force=force, embed=embed)
        stats = sb.store.stats()
        print(f"Synced: {stats['pages']} pages, {stats['journals']} journals, {stats['triples']} triples, {stats['db_size_mb']:.1f}MB")

    # ── status ───────────────────────────────────────────────
    elif cmd == "status":
        sb.print_status()

    # ── stack ────────────────────────────────────────────────
    elif cmd == "stack":
        query = " ".join(sys.argv[2:]) if len(sys.argv) > 2 else None
        if query:
            print(sb.memory.for_query(query))
        else:
            print(sb.memory.compact())
        usage = sb.memory.token_usage()
        print(f"\n[~{usage['l0_l1_total']} tokens]")

    # ── context (pipe-friendly, no decoration) ───────────────
    elif cmd == "context":
        print(sb.memory.compact())

    # ── gods ─────────────────────────────────────────────────
    elif cmd == "gods":
        n = int(sys.argv[2]) if len(sys.argv) > 2 else 15
        for g in sb.store.god_nodes(top_n=n):
            print(f"  {g['name']:<35} {g['mentions']:>3}x")

    # ── neighbors ────────────────────────────────────────────
    elif cmd == "neighbors":
        entity = " ".join(sys.argv[2:])
        for n in sb.store.entity_neighbors(entity):
            d = "→" if n["subject"] == entity else "←"
            other = n["object"] if n["subject"] == entity else n["subject"]
            print(f"  {d} {other}")

    # ── remember ─────────────────────────────────────────────
    elif cmd == "remember":
        summary = " ".join(sys.argv[2:])
        if not summary:
            print("Usage: superbrain remember \"what happened\"")
            sys.exit(1)
        sb.remember("note", "user", summary)
        print(f"Remembered: {summary[:80]}")

    # ── index ─────────────────────────────────────────────
    elif cmd == "index":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        content = wiki.build_index()
        print(f"Index rebuilt ({content.count(chr(10))} lines)")

    # ── lint ──────────────────────────────────────────────
    elif cmd == "lint":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        wiki.print_lint()

    # ── compile ───────────────────────────────────────────
    elif cmd == "compile":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        journal_date = sys.argv[2] if len(sys.argv) > 2 and not sys.argv[2].startswith("-") else None
        dry_run = "--dry-run" in sys.argv
        result = wiki.compile_journal(journal_date, dry_run=dry_run)
        prefix = "[DRY RUN] " if dry_run else ""
        print(f"{prefix}Compiled journal → wiki:")
        for u in result.get("updated", []):
            print(f"  ✓ updated {u['entity']} ({u['entries']} entries)")
        for c in result.get("created", []):
            print(f"  + created {c['entity']} ({c['entries']} entries)")
        skipped = result.get("skipped", [])
        if skipped:
            print(f"  ○ skipped {len(skipped)} entities")
        for conflict in result.get("contradictions", []):
            print(f"  ⚠️  {conflict['entity']}: {conflict['field']} "
                  f"was '{conflict['existing']}' → '{conflict['new']}'")

    # ── compile-all ───────────────────────────────────────
    elif cmd == "compile-all":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        days = int(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2].isdigit() else 7
        dry_run = "--dry-run" in sys.argv
        result = wiki.compile_all(since_days=days, dry_run=dry_run)
        print(f"Compiled {result['journals_processed']} journals: "
              f"{result['total_updated']} updated, {result['total_created']} created")

    # ── save ──────────────────────────────────────────────
    elif cmd == "save":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        if len(sys.argv) < 4:
            print("Usage: superbrain save \"Title\" \"Answer text\" [--query \"original question\"]")
            sys.exit(1)
        title = sys.argv[2]
        answer = sys.argv[3]
        query = ""
        if "--query" in sys.argv:
            qi = sys.argv.index("--query")
            query = sys.argv[qi + 1] if qi + 1 < len(sys.argv) else ""
        path = wiki.save_answer(title, answer, query=query)
        print(f"Saved to: {path}")

    # ── buddy ─────────────────────────────────────────────
    elif cmd == "buddy":
        from core.buddy.buddy import Buddy
        buddy = Buddy()
        buddy.interact()
        subcmd = sys.argv[2] if len(sys.argv) > 2 else "greet"
        if subcmd == "stats":
            print(buddy.stat_card())
        elif subcmd == "status":
            print(buddy.status_line())
        elif subcmd == "pet":
            print(buddy.ascii_art())
            print(buddy.pet())
        elif subcmd == "speak":
            print(buddy.ascii_art())
            print()
            print(buddy.speak())
        elif subcmd == "animate":
            from core.buddy.sprites import SpriteAnimator
            animator = SpriteAnimator(buddy.bones.species)
            print(buddy.greet())
            animator.animate(duration=3.0)
        else:
            print(buddy.ascii_art())
            print()
            print(buddy.greet())

    # ── dream ─────────────────────────────────────────────
    elif cmd == "dream":
        from core.dreams.consolidator import DreamEngine
        engine = DreamEngine()
        if "--status" in sys.argv:
            engine.print_status()
        elif "--force" in sys.argv:
            report = engine.dream(force=True)
            engine.print_report(report)
        else:
            if engine.should_dream():
                report = engine.dream()
                engine.print_report(report)
            else:
                engine.print_status()
                print("Gates not passed. Use --force to dream anyway.")

    # ── setup ─────────────────────────────────────────────
    elif cmd == "setup":
        from core.mcp.setup_mcp import setup_all, check_connections, print_setup_results, print_check_results
        if "--check" in sys.argv:
            results = check_connections()
            print_check_results(results)
        else:
            results = setup_all()
            print_setup_results(results)

    # ── agent ─────────────────────────────────────────────
    elif cmd == "agent":
        from core.agents.scaffold import scaffold_agent, list_agents, agent_info, install_agent, uninstall_agent
        subcmd = sys.argv[2] if len(sys.argv) > 2 else "list"
        if subcmd == "install":
            source = sys.argv[3] if len(sys.argv) > 3 else None
            if not source:
                print("Usage: superbrain agent install <github-url-or-path> [--name custom-name]")
                sys.exit(1)
            name = None
            if "--name" in sys.argv:
                ni = sys.argv.index("--name")
                name = sys.argv[ni + 1] if ni + 1 < len(sys.argv) else None
            print(f"  Installing from {source}...")
            result = install_agent(source, name=name)
            if "error" in result:
                print(f"  Error: {result['error']}")
            else:
                print(f"\n  Agent '{result['name']}' installed!")
                print(f"  Code:  {result['agent_dir']}")
                print(f"  State: {result['state_dir']}")
                print(f"  Deps:  {'installed' if result['deps_installed'] else 'none found'}")
                if result['has_readme']:
                    print(f"  Docs:  {result['agent_dir']}/README.md")
                print(f"\n  Run: superbrain agent info {result['name']}")
                print()
        elif subcmd == "uninstall":
            name = sys.argv[3] if len(sys.argv) > 3 else None
            if not name:
                print("Usage: superbrain agent uninstall <name> [--keep-data]")
                sys.exit(1)
            keep = "--keep-data" in sys.argv
            result = uninstall_agent(name, keep_data=keep)
            if "error" in result:
                print(f"  Error: {result['error']}")
            else:
                print(f"  Agent '{name}' uninstalled.")
                if result['data_kept']:
                    print(f"  State data kept at ~/MAKAKOO/data/{name}/")
                elif result['data_removed']:
                    print(f"  State data removed.")
        elif subcmd == "create":
            name = sys.argv[3] if len(sys.argv) > 3 else None
            if not name:
                print("Usage: superbrain agent create <name> [--description '...'] [--pattern daemon|cron|cli] [--interval 30m]")
                sys.exit(1)
            desc = ""
            pattern = "cli"
            interval = ""
            deps = ""
            args_list = sys.argv[4:]
            i = 0
            while i < len(args_list):
                if args_list[i] == "--description" and i + 1 < len(args_list):
                    desc = args_list[i + 1]; i += 2
                elif args_list[i] == "--pattern" and i + 1 < len(args_list):
                    pattern = args_list[i + 1]; i += 2
                elif args_list[i] == "--interval" and i + 1 < len(args_list):
                    interval = args_list[i + 1]; i += 2
                elif args_list[i] == "--deps" and i + 1 < len(args_list):
                    deps = args_list[i + 1]; i += 2
                else:
                    i += 1
            result = scaffold_agent(name, description=desc, pattern=pattern,
                                    interval=interval, dependencies=deps)
            if "error" in result:
                print(f"Error: {result['error']}")
            else:
                print(f"\n  Agent '{result['name']}' created!")
                print(f"  Code:  {result['agent_dir']}")
                print(f"  State: {result['state_dir']}")
                print(f"  Pattern: {result['pattern']}")
                print(f"\n  Next steps:")
                for step in result.get("next_steps", []):
                    print(f"    {step}")
                print()
        elif subcmd == "list":
            agents = list_agents()
            if not agents:
                print("No agents installed. Create one: superbrain agent create <name>")
            else:
                print(f"\n  {'Name':<25} {'Pattern':<12} {'Status':<12} Description")
                print(f"  {'─'*25} {'─'*12} {'─'*12} {'─'*30}")
                for a in agents:
                    print(f"  {a['name']:<25} {a.get('pattern', '?'):<12} {a.get('status', '?'):<12} {a.get('description', '')[:40]}")
                print()
        elif subcmd == "info":
            name = sys.argv[3] if len(sys.argv) > 3 else None
            if name:
                print(agent_info(name))
            else:
                print("Usage: superbrain agent info <name>")
        else:
            print(f"Unknown: agent {subcmd}. Try: create, list, info")

    # ── nursery ───────────────────────────────────────────
    elif cmd == "nursery":
        from core.buddy.nursery import Nursery
        nursery = Nursery()
        subcmd = sys.argv[2] if len(sys.argv) > 2 else "roll_call"
        if subcmd == "hatch":
            trigger = sys.argv[3] if len(sys.argv) > 3 else "random"
            context = " ".join(sys.argv[4:]) if len(sys.argv) > 4 else ""
            baby = nursery.hatch(trigger=trigger, context=context)
            print(nursery.show_mascot(baby.mascot_id))
            print(f"Welcome to the family, {baby.name}! 🎉")
        elif subcmd == "show":
            name = " ".join(sys.argv[3:]) if len(sys.argv) > 3 else ""
            if name:
                print(nursery.show_mascot(name))
            else:
                print("Usage: superbrain nursery show <name or id>")
        elif subcmd == "feed":
            print(nursery.feed_all())
        elif subcmd == "mood":
            mood = nursery.family_mood()
            print(f"\n  Family Psych Level: {mood['level'].upper()}")
            print(f"  Score: {mood['score']}/100")
            print(f"  Mascots: {mood['total_mascots']}")
            print(f"  {mood['message']}\n")
        else:
            print(nursery.roll_call())

    # ── sancho ────────────────────────────────────────────
    elif cmd == "sancho":
        from core.sancho.engine import Sancho
        sancho = Sancho()
        subcmd = sys.argv[2] if len(sys.argv) > 2 else "status"
        if subcmd == "status":
            sancho.print_status()
        elif subcmd == "tick":
            results = sancho.tick()
            if not results:
                print("  No eligible tasks this tick.")
            for r in results:
                name = r.get('task', r.get('name', '?'))
                dur = r.get('duration', r.get('duration_sec', 0))
                print(f"  [{name}] {r.get('summary', r.get('status', 'done'))} ({dur:.1f}s)")
        elif subcmd == "enable":
            print("SANCHO enabled. Run 'superbrain sancho tick' or set up cron.")
        elif subcmd == "disable":
            print("SANCHO disabled.")
        else:
            sancho.print_status()

    # ── swarm ─────────────────────────────────────────────
    elif cmd == "swarm":
        from core.coordinator.coordinator import Coordinator
        subcmd = sys.argv[2] if len(sys.argv) > 2 else "list"
        coord = Coordinator()
        if subcmd == "list":
            tasks = coord.list_tasks()
            if not tasks:
                print("No swarm tasks.")
            for t in tasks:
                print(f"  [{t['task_id']}] {t['status']} — {t['objective'][:60]}")
        elif subcmd == "status":
            task_id = sys.argv[3] if len(sys.argv) > 3 else None
            if task_id:
                t = coord.get_task(task_id)
                if t:
                    print(json.dumps(t, indent=2, default=str))
                else:
                    print(f"Task {task_id} not found.")
            else:
                print("Usage: superbrain swarm status <task_id>")
        elif subcmd == "cancel":
            task_id = sys.argv[3] if len(sys.argv) > 3 else None
            if task_id:
                coord.cancel_task(task_id)
                print(f"Cancelled {task_id}")
            else:
                print("Usage: superbrain swarm cancel <task_id>")
        else:
            # Treat as objective
            objective = " ".join(sys.argv[2:])
            print(f"Launching swarm: {objective}")
            result = coord.execute(objective)
            print(f"\nSwarm {result.task_id} completed: {result.status}")
            print(f"Scratchpad: {result.scratchpad_dir}")

    # ── costs ─────────────────────────────────────────────
    elif cmd == "costs":
        from core.telemetry.cost_tracker import CostTracker
        if "--history" in sys.argv:
            days = 30
            for a in sys.argv:
                if a.isdigit():
                    days = int(a)
            CostTracker.print_history(days)
        else:
            tracker = CostTracker()
            tracker.print_session()

    else:
        print(f"Unknown: {cmd}. Run 'superbrain --help'")
        sys.exit(1)


if __name__ == "__main__":
    main()
