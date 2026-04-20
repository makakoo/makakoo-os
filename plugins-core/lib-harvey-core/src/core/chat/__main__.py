#!/usr/local/opt/python@3.11/bin/python3.11
"""
HarveyChat CLI — Start, stop, and manage the chat gateway.

Usage:
    python3 -m core.chat start           # Start in foreground
    python3 -m core.chat start --daemon   # Start as background daemon
    python3 -m core.chat stop             # Stop running daemon
    python3 -m core.chat status           # Check if running
    python3 -m core.chat config           # Show current config
    python3 -m core.chat setup            # Interactive setup wizard
"""

import argparse
import asyncio
import json
import logging
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))

from core.chat.config import load_config, save_config, CHAT_DATA_DIR, CONFIG_PATH
from core.chat.gateway import HarveyChat

LOG_FILE = CHAT_DATA_DIR / "harveychat.log"


def setup_logging(daemon: bool = False):
    CHAT_DATA_DIR.mkdir(parents=True, exist_ok=True)
    handlers = [logging.StreamHandler()]
    if daemon:
        handlers = [logging.FileHandler(LOG_FILE)]

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
        datefmt="%H:%M:%S",
        handlers=handlers,
    )


def cmd_start(args):
    """Start the gateway."""
    cfg = load_config()

    if not cfg.telegram.bot_token:
        print("HarveyChat is not configured yet.")
        print()
        if sys.stdin.isatty():
            if _ask_yn("Run the setup wizard now?"):
                cmd_setup(args)
                return
        print("Run the setup wizard first:")
        print(f"  {sys.executable} -m core.chat setup")
        sys.exit(1)

    if args.daemon:
        # Fork to background
        pid_path = Path(cfg.pid_file)
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
            except (ValueError, OSError):
                pid = None
            if pid is not None:
                try:
                    os.kill(pid, 0)
                    print(f"HarveyChat already running (PID {pid})")
                    sys.exit(1)
                except OSError:
                    # Stale PID file — process is dead, clean up
                    print(f"Cleaning up stale PID file (PID {pid} is dead)")
                    pid_path.unlink(missing_ok=True)
            else:
                pid_path.unlink(missing_ok=True)

        # Log rotation: rename old log, start fresh if >10MB
        if LOG_FILE.exists() and LOG_FILE.stat().st_size > 10 * 1024 * 1024:
            try:
                rotated = LOG_FILE.with_suffix(".log.1")
                if rotated.exists():
                    rotated.unlink()  # keep only one rotated copy
                LOG_FILE.rename(rotated)
                LOG_FILE.write_text(
                    f"--- Log rotated at {time.strftime('%Y-%m-%d %H:%M:%S')} "
                    f"(previous log: {rotated.name}) ---\n"
                )
            except OSError:
                pass

        print("Starting HarveyChat daemon...")
        # Re-run self in background
        cmd = [sys.executable, "-m", "core.chat", "start"]
        env = os.environ.copy()
        proc = subprocess.Popen(
            cmd,
            stdout=open(LOG_FILE, "a"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
            cwd=os.path.join(HARVEY_HOME, "harvey-os"),
            env=env,
        )
        # Wait briefly for startup
        time.sleep(2)
        if proc.poll() is None:
            print(f"HarveyChat started (PID {proc.pid})")
            print(f"Log: {LOG_FILE}")
        else:
            print("HarveyChat failed to start. Check log:")
            print(f"  tail -f {LOG_FILE}")
            sys.exit(1)
        return

    # Foreground mode
    setup_logging(daemon=False)
    print("Starting HarveyChat gateway (Ctrl+C to stop)...")
    print(
        f"Channels: Telegram {'configured' if cfg.telegram.bot_token else 'NOT configured'}"
    )
    print(f"Bridge: switchAILocal → {cfg.bridge.switchai_url}")
    print()

    chat = HarveyChat(cfg)

    # Handle signals
    loop = asyncio.new_event_loop()

    def shutdown(sig, frame):
        print("\nShutting down...")
        loop.call_soon_threadsafe(lambda: asyncio.ensure_future(chat.stop()))

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)

    loop.run_until_complete(chat.start())


def cmd_stop(args):
    """Stop the daemon."""
    cfg = load_config()
    pid_path = Path(cfg.pid_file)

    if not pid_path.exists():
        print("HarveyChat is not running (no PID file)")
        return

    try:
        pid = int(pid_path.read_text().strip())
    except (ValueError, OSError):
        print("Invalid PID file — cleaning up")
        pid_path.unlink(missing_ok=True)
        return

    try:
        os.kill(pid, signal.SIGTERM)
        print(f"Sent SIGTERM to PID {pid}")
        # Wait for clean shutdown — 15s grace period (was 5s)
        for _ in range(30):
            time.sleep(0.5)
            try:
                os.kill(pid, 0)
            except OSError:
                print("HarveyChat stopped.")
                pid_path.unlink(missing_ok=True)
                return
        print("Process still running after 15s — sending SIGKILL")
        os.kill(pid, signal.SIGKILL)
        time.sleep(1)
        pid_path.unlink(missing_ok=True)
        print("HarveyChat killed.")
    except OSError:
        print(f"Process {pid} not found — cleaning up PID file")
        pid_path.unlink(missing_ok=True)


def cmd_status(args):
    """Check gateway status."""
    cfg = load_config()
    pid_path = Path(cfg.pid_file)

    if pid_path.exists():
        pid = int(pid_path.read_text().strip())
        try:
            os.kill(pid, 0)
            print(f"HarveyChat is RUNNING (PID {pid})")

            # Show store stats
            from core.chat.store import ChatStore

            store = ChatStore(cfg.db_path)
            stats = store.get_stats()
            print(f"  Messages: {stats['total_messages']}")
            print(f"  Sessions: {stats['total_sessions']}")
            print(f"  Channels: {', '.join(stats['channels']) or 'none yet'}")
            store.close()
            return
        except OSError:
            pass

    print("HarveyChat is NOT running")
    print(f"  Config: {CONFIG_PATH}")
    print(f"  Telegram: {'configured' if cfg.telegram.bot_token else 'NOT configured'}")


def cmd_config(args):
    """Show current configuration."""
    cfg = load_config()
    token = cfg.telegram.bot_token
    masked = (
        f"{token[:8]}...{token[-4:]}"
        if len(token) > 12
        else ("set" if token else "NOT SET")
    )

    print("HarveyChat Configuration")
    print(f"  Config file: {CONFIG_PATH}")
    print(f"  Database: {cfg.db_path}")
    print()
    print("Telegram:")
    print(f"  Bot token: {masked}")
    print(f"  Allowed users: {cfg.telegram.allowed_user_ids or 'all'}")
    print(f"  Polling timeout: {cfg.telegram.polling_timeout}s")
    print()
    print("Bridge:")
    print(f"  switchAILocal: {cfg.bridge.switchai_url}")
    print(f"  Model: {cfg.bridge.switchai_model}")
    print(
        f"  Anthropic fallback: {'configured' if cfg.bridge.anthropic_api_key else 'not set'}"
    )
    print(f"  Max history: {cfg.bridge.max_history_messages} messages")
    print()
    print(f"Brain sync: {'on' if cfg.log_to_brain else 'off'}")


def _clear():
    os.system("cls" if os.name == "nt" else "clear")


def _box(text, width=60):
    """Draw a box around text."""
    lines = text.split("\n")
    top = "+" + "-" * (width - 2) + "+"
    bot = top
    mid = []
    for line in lines:
        padded = line.ljust(width - 4)[: width - 4]
        mid.append(f"| {padded} |")
    return "\n".join([top] + mid + [bot])


def _ask(prompt, default="", secret=False, required=False):
    """Ask a question with a default value. Loop until answered if required."""
    suffix = ""
    if default:
        suffix = f" [{default}]"
    elif not required:
        suffix = " (press Enter to skip)"

    while True:
        try:
            answer = input(f"  > {prompt}{suffix}: ").strip()
        except (EOFError, KeyboardInterrupt):
            print()
            sys.exit(0)

        if not answer and default:
            return default
        if not answer and not required:
            return ""
        if not answer and required:
            print("    This field is required. Please enter a value.")
            continue
        return answer


def _ask_yn(prompt, default=True):
    """Yes/no question."""
    hint = "[Y/n]" if default else "[y/N]"
    try:
        answer = input(f"  > {prompt} {hint}: ").strip().lower()
    except (EOFError, KeyboardInterrupt):
        print()
        sys.exit(0)
    if not answer:
        return default
    return answer in ("y", "yes", "si", "ja", "da")


def _wait_enter(msg="Press Enter to continue..."):
    try:
        input(f"\n  {msg}")
    except (EOFError, KeyboardInterrupt):
        print()
        sys.exit(0)


def _check_switchai():
    """Check if switchAILocal is reachable."""
    try:
        import requests

        r = requests.get("http://localhost:18080/health", timeout=3)
        return r.status_code == 200
    except Exception:
        return False


def _check_and_install_deps() -> bool:
    """Check and install HarveyChat dependencies."""
    import importlib
    import subprocess

    required = {
        "telegram": "python-telegram-bot",
        "requests": "requests",
        "whisper": "faster-whisper",
    }
    missing = []
    for name, pkg in required.items():
        try:
            importlib.import_module(name)
        except ImportError:
            missing.append(pkg)

    # Check system tools
    import shutil

    system_tools = ["ffmpeg", "tesseract", "pdftotext"]
    missing_tools = [t for t in system_tools if not shutil.which(t)]

    if not missing and not missing_tools:
        print("  All dependencies installed.")
        return True

    if missing:
        print(f"  Missing Python packages: {', '.join(missing)}")
        print("  Installing...")

        # Find pip
        pip = shutil.which("pip3") or shutil.which("pip")
        if not pip:
            pip = sys.executable + " -m pip"

        req_file = Path(__file__).parent / "requirements.txt"
        if req_file.exists():
            result = subprocess.run(
                [sys.executable, "-m", "pip", "install", "-q", "-r", str(req_file)],
                capture_output=True,
                text=True,
                timeout=120,
            )
            if result.returncode != 0:
                print(f"  Warning: pip install failed: {result.stderr[:200]}")
            else:
                print("  Python packages installed.")
        else:
            print("  requirements.txt not found — install manually:")
            print(f"    pip install {' '.join(missing)}")

    if missing_tools:
        print(f"  Missing system tools: {', '.join(missing_tools)}")
        if not _ask_yn("Install system tools with Homebrew?", default=True):
            print("  Install manually:")
            print(f"    brew install {' '.join(missing_tools)}")
            return False

        result = subprocess.run(
            ["brew", "install"] + missing_tools,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if result.returncode != 0:
            print(f"  Homebrew install failed: {result.stderr[:200]}")
            return False

        print("  System tools installed.")

    return True


def _validate_token(token):
    """Quick validation of a Telegram bot token format."""
    if ":" not in token:
        return False
    parts = token.split(":")
    if len(parts) != 2:
        return False
    try:
        int(parts[0])
    except ValueError:
        return False
    return len(parts[1]) > 20


def _test_telegram_token(token):
    """Actually test the token against Telegram API."""
    try:
        import requests

        r = requests.get(
            f"https://api.telegram.org/bot{token}/getMe",
            timeout=10,
        )
        if r.status_code == 200:
            data = r.json()
            if data.get("ok"):
                bot = data["result"]
                return True, bot.get("first_name", ""), bot.get("username", "")
        return False, "", ""
    except Exception:
        return False, "", ""


def cmd_setup(args):
    """Interactive setup wizard — grandma-proof."""
    cfg = load_config()
    is_reconfigure = bool(cfg.telegram.bot_token)
    goto_step2 = False
    bot_username = ""

    _clear()
    print()
    print(
        _box(
            "HARVEYCHAT SETUP WIZARD\n"
            "\n"
            "Talk to Harvey from your phone.\n"
            "This takes about 2 minutes.\n"
            "\n"
            "You'll need:\n"
            "  - Telegram app on your phone\n"
            "  - That's it!"
        )
    )
    print()
    _wait_enter("Ready? Press Enter to start...")

    # ── STEP 0: Install dependencies ─────────────────────────
    _clear()
    print()
    print("=" * 60)
    print("  STEP 0: Installing dependencies")
    print("=" * 60)
    print()

    deps_ok = _check_and_install_deps()

    if not deps_ok:
        if not _ask_yn("Continue anyway? (Some features may not work)", default=False):
            print("  Run this again after installing dependencies:")
            print("    pip install -r ~/MAKAKOO/harvey-os/core/chat/requirements.txt")
            return

    # ── STEP 1: Create the Telegram bot ──────────────────
    _clear()
    print()
    print("=" * 60)
    print("  STEP 1 of 4: Create your Telegram bot")
    print("=" * 60)
    print()

    if is_reconfigure:
        print("  You already have a bot configured.")
        if not _ask_yn("Do you want to set up a new one?", default=False):
            print("  Keeping existing bot.")
            goto_step2 = True

    if not goto_step2:
        print("  Follow these steps on your phone or computer:")
        print()
        print("  1. Open Telegram")
        print("  2. Search for:  @BotFather")
        print("  3. Tap on BotFather (the one with the blue checkmark)")
        print("  4. Send this message:  /newbot")
        print("  5. When asked for a name, type:  Harvey")
        print("  6. When asked for a username, type something like:")
        print("     harvey_myname_bot  (must end in 'bot')")
        print("  7. BotFather will give you a token that looks like:")
        print("     1234567890:ABCdefGHIjklMNOpqrsTUVwxyz")
        print()
        print("  Copy that token and paste it here.")
        print()

        while True:
            token = _ask("Paste your bot token here", required=True)

            if not _validate_token(token):
                print()
                print("    That doesn't look like a valid token.")
                print("    It should look like: 1234567890:ABCdefGHI...")
                print("    (a number, then a colon, then a long string)")
                print()
                continue

            print()
            print("  Checking token with Telegram...")
            ok, bot_name, bot_username = _test_telegram_token(token)

            if ok:
                print(f"  Connected to: {bot_name} (@{bot_username})")
                print()
                cfg.telegram.bot_token = token
                break
            else:
                print("    Token didn't work. Telegram rejected it.")
                print("    Double-check you copied the full token from BotFather.")
                print()
                if not _ask_yn("Try again?"):
                    print("  Setup cancelled.")
                    return

    # ── STEP 2: Security — who can talk to Harvey ────────
    _clear()
    print()
    print("=" * 60)
    print("  STEP 2 of 4: Security")
    print("=" * 60)
    print()
    print("  Right now, ANYONE who finds your bot can talk to Harvey.")
    print("  Let's lock it down so only YOU can use it.")
    print()

    if cfg.telegram.allowed_user_ids:
        print(f"  Currently allowed: {cfg.telegram.allowed_user_ids}")
        if not _ask_yn("Update the allowed users?", default=False):
            print("  Keeping existing security settings.")
        else:
            cfg.telegram.allowed_user_ids = []  # reset, will re-ask below

    if not cfg.telegram.allowed_user_ids:
        print("  To find your Telegram user ID:")
        print()
        print("  1. Open Telegram")
        print("  2. Search for:  @userinfobot")
        print("  3. Send it any message")
        print("  4. It will reply with your user ID (a number)")
        print()

        user_id = _ask("Paste your Telegram user ID (the number)")

        if user_id:
            try:
                uid = int(user_id.strip())
                cfg.telegram.allowed_user_ids = [uid]
                print(f"\n  Only user {uid} can talk to Harvey now.")
            except ValueError:
                print("  That's not a number. Skipping — anyone can message your bot.")
                print("  You can fix this later by running setup again.")
        else:
            print()
            print("  Skipped. WARNING: Anyone can message your bot!")
            print("  Run setup again to fix this later.")

    # ── STEP 3: AI Brain connection ──────────────────────
    _clear()
    print()
    print("=" * 60)
    print("  STEP 3 of 4: Connect Harvey's brain")
    print("=" * 60)
    print()

    switchai_ok = _check_switchai()

    if switchai_ok:
        print("  switchAILocal detected at localhost:18080")
        print("  Harvey will use your local AI gateway. No extra setup needed.")
        print()

        # Offer model selection
        print("  Which model should Harvey use for chat responses?")
        print()
        print("  Popular choices:")
        print("    1. minimax:MiniMax-M2.7                 (recommended — fast, smart)")
        print("    2. anthropic:claude-sonnet-4-20250514  (smart, fast)")
        print("    3. anthropic:claude-opus-4-20250514    (smartest, slower)")
        print("    4. google:gemini-2.5-flash             (fast, cheap)")
        print("    5. google:gemini-2.5-pro               (smart, fast)")
        print("    6. Keep current: " + cfg.bridge.switchai_model)
        print()

        choice = _ask("Pick a number or type a model name", default="6")
        models = {
            "1": "minimax:MiniMax-M2.7",
            "2": "anthropic:claude-sonnet-4-20250514",
            "3": "anthropic:claude-opus-4-20250514",
            "4": "google:gemini-2.5-flash",
            "5": "google:gemini-2.5-pro",
        }
        if choice in models:
            cfg.bridge.switchai_model = models[choice]
            print(f"  Using: {cfg.bridge.switchai_model}")
        elif choice != "6":
            cfg.bridge.switchai_model = choice
            print(f"  Using custom model: {choice}")
        else:
            print(f"  Keeping: {cfg.bridge.switchai_model}")

    else:
        print("  switchAILocal is not running.")
        print("  Harvey needs an AI backend to think. Two options:")
        print()
        print("  Option A: Start switchAILocal (recommended)")
        print("    Run:  ail.sh start")
        print("    Then run this setup again.")
        print()
        print("  Option B: Use Anthropic API directly")
        print("    You'll need an API key from console.anthropic.com")
        print()

        if _ask_yn("Do you have an Anthropic API key?", default=False):
            api_key = _ask("Paste your Anthropic API key", required=True)
            cfg.bridge.anthropic_api_key = api_key
            print("  Anthropic API configured as fallback.")
        else:
            print()
            print("  No AI backend configured.")
            print("  Start switchAILocal before running HarveyChat:")
            print("    ail.sh start")

    # ── SAVE & LAUNCH ────────────────────────────────────
    save_config(cfg)

    _clear()
    print()
    print(
        _box(
            "SETUP COMPLETE!\n"
            "\n"
            "Your config has been saved.\n"
            f"Bot: @{bot_username or 'configured'}\n"
            f"Security: {len(cfg.telegram.allowed_user_ids)} allowed user(s)\n"
            f"AI: {cfg.bridge.switchai_model}\n"
            f"Brain sync: on"
        )
    )
    print()
    print(f"  Config saved to: {CONFIG_PATH}")
    print()

    if cfg.telegram.bot_token:
        if _ask_yn("Start Harvey Chat now?"):
            print()
            if _ask_yn("Run in background (daemon mode)?"):
                print()
                print("  Starting HarveyChat daemon...")

                # Fake args for cmd_start
                class DaemonArgs:
                    daemon = True

                cmd_start(DaemonArgs())
            else:
                print()
                print("  Starting HarveyChat (Ctrl+C to stop)...")
                print("  Open Telegram and message your bot!")
                print()

                class FgArgs:
                    daemon = False

                cmd_start(FgArgs())
        else:
            print("  To start later:")
            print(f"    cd ~/MAKAKOO/harvey-os")
            print(f"    {sys.executable} -m core.chat start --daemon")
            print()
            print("  Then open Telegram and message your bot!")
    else:
        print("  Setup incomplete — no bot token configured.")
        print("  Run setup again when ready.")


def cmd_doctor(args):
    """Run a diagnostic report on the Telegram setup."""
    from core.chat.telegram_utils import diagnose

    cfg = load_config()
    print("HarveyChat — Telegram diagnostic")
    print("=" * 60)

    report = diagnose(cfg.telegram, check_network=True)
    for r in report.results:
        status = "✓" if r.ok else "✗"
        print(f"  [{status}] {r.name}: {r.detail}")

    print("=" * 60)
    if report.all_ok():
        print("  All checks passed.")
    else:
        print(f"  {len(report.failed())} check(s) failed — see details above.")
        print()
        print("  Common fixes:")
        print("    - `python3 -m core.chat setup` to (re)configure the bot token")
        print("    - `python3 -m core.chat add-chat` to register a group")
        print("    - `python3 -m core.chat detect` to pull IDs from getUpdates")
        sys.exit(1)


def cmd_detect(args):
    """Pull recent updates from getUpdates and show every user/chat we saw."""
    from core.chat.telegram_utils import fetch_recent_updates, normalize_chat_id

    cfg = load_config()
    if not cfg.telegram.bot_token:
        print("  No bot token configured. Run `setup` first.")
        sys.exit(1)

    print("  Open Telegram → message your bot OR send a message in your group.")
    print("  Then press Enter to pull recent updates...")
    try:
        input()
    except (EOFError, KeyboardInterrupt):
        print()
        return

    updates = fetch_recent_updates(cfg.telegram.bot_token)
    if not updates:
        print("  No updates received.")
        print("  Tips:")
        print("    - Make sure you messaged the bot AFTER it started polling")
        print("    - If HarveyChat is running, stop it first (`stop`) or")
        print("      the polling worker is already consuming updates.")
        return

    # Distil unique (chat, user) pairs
    seen = {}
    for u in updates:
        key = (u["chat_id"], u["user_id"])
        if key not in seen:
            seen[key] = u

    print(f"  Detected {len(seen)} unique sender(s):")
    print()
    for (chat_id, user_id), u in seen.items():
        chat_type = u["chat_type"]
        chat_title = u["chat_title"] or "(no title)"
        username = u["username"] or "(no username)"
        text_preview = u["text"][:60]
        normalized = normalize_chat_id(chat_id)
        bot_tag = " [BOT]" if u["is_bot"] else ""
        print(f"    user_id={user_id}  @{username}{bot_tag}")
        print(f"    chat_id={chat_id}  ({chat_type}: {chat_title})")
        if normalized != chat_id:
            print(f"      → normalized: {normalized}")
        if text_preview:
            print(f"    text: {text_preview!r}")
        print()

    print("  Next steps:")
    print("    - Copy the user_id to add to allowed_user_ids (DMs)")
    print("    - Copy the chat_id to add to allowed_chat_ids (groups)")
    print("    - Or run `add-chat` to do it interactively")


def cmd_add_chat(args):
    """Interactively register a chat/group + optionally sync the Claude plugin."""
    from core.chat.telegram_utils import (
        claude_plugin_installed,
        normalize_chat_id,
        sync_claude_plugin_access,
    )

    cfg = load_config()

    print()
    print("=" * 60)
    print("  ADD CHAT — register a group or extra DM with Harvey")
    print("=" * 60)
    print()
    print("  Paste the chat_id. For groups, either form works:")
    print("    - Bare peer id: 3746642416 (we'll add the -100 prefix)")
    print("    - Full Bot API id: -1003746642416")
    print()
    print("  Tip: run `detect` first to find the id automatically.")
    print()

    raw = _ask("Chat id", required=True)
    try:
        chat_id = normalize_chat_id(raw)
    except (ValueError, TypeError) as e:
        print(f"  ✗ Could not parse chat id: {e}")
        sys.exit(1)

    if chat_id != (int(raw) if str(raw).lstrip("-").isdigit() else raw):
        print(f"  Normalized to {chat_id}")

    if chat_id in cfg.telegram.allowed_chat_ids:
        print(f"  Chat {chat_id} is already in the allowlist.")
    else:
        cfg.telegram.allowed_chat_ids.append(chat_id)
        print(f"  ✓ Added {chat_id} to HarveyChat allowed_chat_ids")

    # Optionally add specific user ids that should be allowed in this chat
    print()
    if _ask_yn(
        "Restrict this chat to specific users? (No = anyone in the group)",
        default=False,
    ):
        ids_raw = _ask(
            "Comma-separated user ids",
            required=True,
        )
        try:
            new_users = [int(x.strip()) for x in ids_raw.split(",") if x.strip()]
        except ValueError:
            print("  Invalid user ids — skipping.")
            new_users = []
        for uid in new_users:
            if uid not in cfg.telegram.allowed_user_ids:
                cfg.telegram.allowed_user_ids.append(uid)
                print(f"  ✓ Added user {uid} to allowed_user_ids")
    else:
        new_users = list(cfg.telegram.allowed_user_ids)

    save_config(cfg)
    print(f"  ✓ Saved to {CONFIG_PATH}")

    # Claude Code plugin sync (if installed)
    if claude_plugin_installed():
        print()
        print("  Claude Code Telegram plugin detected.")
        if _ask_yn(
            "Sync this chat to the plugin's access.json too?",
            default=True,
        ):
            ok, msg = sync_claude_plugin_access(
                chat_id=chat_id,
                user_ids=new_users or cfg.telegram.allowed_user_ids,
                require_mention=False,
                dm_user_ids=cfg.telegram.allowed_user_ids,
            )
            if ok:
                print(f"  ✓ {msg}")
            else:
                print(f"  ✗ {msg}")
    else:
        print()
        print("  (Claude Code Telegram plugin not detected — skipped)")

    print()
    print("  Done. Restart HarveyChat to pick up the new allowlist:")
    print("    python3 -m core.chat stop && python3 -m core.chat start --daemon")


def cmd_sync_claude(args):
    """Re-sync all currently-allowed chat ids to the Claude plugin."""
    from core.chat.telegram_utils import (
        claude_plugin_installed,
        sync_claude_plugin_access,
    )

    cfg = load_config()
    if not claude_plugin_installed():
        print("  Claude Code Telegram plugin is not installed.")
        print("    (~/.claude/channels/telegram/ is missing)")
        sys.exit(1)

    if not cfg.telegram.allowed_chat_ids:
        print("  No chats in allowed_chat_ids to sync.")
        sys.exit(0)

    for chat_id in cfg.telegram.allowed_chat_ids:
        ok, msg = sync_claude_plugin_access(
            chat_id=chat_id,
            user_ids=cfg.telegram.allowed_user_ids,
            require_mention=False,
            dm_user_ids=cfg.telegram.allowed_user_ids,
        )
        status = "✓" if ok else "✗"
        print(f"  [{status}] {msg}")
    print("  Done.")


def main():
    parser = argparse.ArgumentParser(
        description="HarveyChat — External communication gateway for Harvey OS"
    )
    sub = parser.add_subparsers(dest="command")

    start_p = sub.add_parser("start", help="Start the gateway")
    start_p.add_argument(
        "--daemon", "-d", action="store_true", help="Run as background daemon"
    )

    sub.add_parser("stop", help="Stop the gateway daemon")
    sub.add_parser("status", help="Check gateway status")
    sub.add_parser("config", help="Show configuration")
    sub.add_parser("setup", help="Interactive setup wizard")
    sub.add_parser("doctor", help="Run diagnostic report on Telegram setup")
    sub.add_parser("detect", help="Pull recent getUpdates to find user/chat ids")
    sub.add_parser("add-chat", help="Interactively add a group/chat to the allowlist")
    sub.add_parser("sync-claude", help="Sync allowed chats to the Claude Code plugin")

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    commands = {
        "start": cmd_start,
        "stop": cmd_stop,
        "status": cmd_status,
        "config": cmd_config,
        "setup": cmd_setup,
        "doctor": cmd_doctor,
        "detect": cmd_detect,
        "add-chat": cmd_add_chat,
        "sync-claude": cmd_sync_claude,
    }

    commands[args.command](args)


if __name__ == "__main__":
    main()
