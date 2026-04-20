#!/usr/bin/env python3
"""
Hacker News Monitor — fetches top stories, notifies new ones via Telegram & email.

Runs hourly (cron or --daemon mode). Tracks seen stories in a JSON index
so only genuinely new stories trigger notifications.

Usage:
    python3 hn_monitor.py              # Run once (check for new stories)
    python3 hn_monitor.py --daemon     # Run every hour in a loop
    python3 hn_monitor.py --test       # Fetch stories and send one test notification
"""

import json
import logging
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Dict, List, Optional

import requests

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [hn-monitor] %(levelname)s: %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("hn-monitor")

# ── Config ──────────────────────────────────────────────
HN_API = "https://hacker-news.firebaseio.com/v0"
DATA_DIR = Path(HARVEY_HOME) / "data" / "hackernews"
SEEN_PATH = DATA_DIR / "seen.json"
TOP_K = 30  # how many top stories to check
MIN_SCORE = 10  # only notify stories above this score
CHECK_INTERVAL = 3600  # 1 hour in seconds


# ── HN API ──────────────────────────────────────────────

def fetch_top_story_ids(limit: int = TOP_K) -> List[int]:
    """Fetch top story IDs from HN."""
    resp = requests.get(f"{HN_API}/topstories.json", timeout=10)
    resp.raise_for_status()
    return resp.json()[:limit]


def fetch_item(item_id: int) -> Optional[Dict]:
    """Fetch a single HN item."""
    try:
        resp = requests.get(f"{HN_API}/item/{item_id}.json", timeout=10)
        resp.raise_for_status()
        return resp.json()
    except Exception as e:
        log.warning(f"Failed to fetch item {item_id}: {e}")
        return None


def fetch_top_stories(limit: int = TOP_K) -> List[Dict]:
    """Fetch top stories with details."""
    ids = fetch_top_story_ids(limit)
    stories = []
    for sid in ids:
        item = fetch_item(sid)
        if item and item.get("type") == "story" and not item.get("dead"):
            stories.append(item)
    return stories


# ── Seen Index ──────────────────────────────────────────

def load_seen() -> Dict[str, dict]:
    """Load seen stories index. Keys are string IDs."""
    if SEEN_PATH.exists():
        try:
            return json.loads(SEEN_PATH.read_text())
        except Exception:
            pass
    return {}


def save_seen(seen: Dict[str, dict]):
    """Persist seen index."""
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    SEEN_PATH.write_text(json.dumps(seen, indent=2))


def find_new_stories(stories: List[Dict], seen: Dict[str, dict]) -> List[Dict]:
    """Return stories not in the seen index, above MIN_SCORE."""
    new = []
    for s in stories:
        sid = str(s["id"])
        score = s.get("score", 0)
        if sid not in seen and score >= MIN_SCORE:
            new.append(s)
    return new


def mark_seen(stories: List[Dict], seen: Dict[str, dict]) -> Dict[str, dict]:
    """Mark stories as seen. Keep last 1000 entries to prevent unbounded growth."""
    for s in stories:
        seen[str(s["id"])] = {
            "title": s.get("title", ""),
            "score": s.get("score", 0),
            "seen_at": int(time.time()),
        }
    # Prune: keep only the 1000 most recently seen
    if len(seen) > 1000:
        sorted_entries = sorted(seen.items(), key=lambda x: x[1].get("seen_at", 0), reverse=True)
        seen = dict(sorted_entries[:1000])
    return seen


# ── Notifications ───────────────────────────────────────

def format_stories_message(stories: List[Dict]) -> str:
    """Format stories for Telegram/email notification."""
    lines = [f"🔶 *{len(stories)} new on Hacker News*\n"]
    for i, s in enumerate(stories, 1):
        title = s.get("title", "(no title)")
        url = s.get("url", f"https://news.ycombinator.com/item?id={s['id']}")
        score = s.get("score", 0)
        comments = s.get("descendants", 0)
        hn_link = f"https://news.ycombinator.com/item?id={s['id']}"
        lines.append(f"{i}. *{title}*")
        lines.append(f"   ↑{score} | 💬{comments} | [link]({url}) | [comments]({hn_link})")
    return "\n".join(lines)


def send_telegram(message: str) -> bool:
    """Send notification to Telegram via the bot."""
    try:
        # Load bot config
        config_path = Path(HARVEY_HOME) / "data" / "chat" / "config.json"
        if not config_path.exists():
            log.warning("No chat config found — Telegram not configured")
            return False

        config = json.loads(config_path.read_text())
        bot_token = config.get("telegram", {}).get("bot_token", "")
        allowed_users = config.get("telegram", {}).get("allowed_user_ids", [])

        if not bot_token:
            log.warning("No Telegram bot token configured")
            return False

        if not allowed_users:
            log.warning("No allowed Telegram users configured — can't send outbound")
            return False

        # Send to all allowed users
        sent = 0
        for user_id in allowed_users:
            try:
                resp = requests.post(
                    f"https://api.telegram.org/bot{bot_token}/sendMessage",
                    json={
                        "chat_id": user_id,
                        "text": message,
                        "parse_mode": "Markdown",
                        "disable_web_page_preview": True,
                    },
                    timeout=10,
                )
                if resp.status_code == 200:
                    sent += 1
                    log.info(f"Telegram sent to user {user_id}")
                else:
                    # Retry without markdown if parsing fails
                    resp2 = requests.post(
                        f"https://api.telegram.org/bot{bot_token}/sendMessage",
                        json={
                            "chat_id": user_id,
                            "text": message,
                            "disable_web_page_preview": True,
                        },
                        timeout=10,
                    )
                    if resp2.status_code == 200:
                        sent += 1
                    else:
                        log.warning(f"Telegram send failed for {user_id}: {resp2.text[:200]}")
            except Exception as e:
                log.warning(f"Telegram error for {user_id}: {e}")

        return sent > 0

    except Exception as e:
        log.error(f"Telegram notification failed: {e}")
        return False


def send_email(subject: str, body: str) -> bool:
    """Send notification via gws Gmail."""
    try:
        env = os.environ.copy()
        extra_paths = ["/usr/local/bin", "/opt/homebrew/bin",
                       os.path.expanduser("~/.nvm/versions/node/v22.17.0/bin"),
                       os.path.expanduser("~/bin")]
        env["PATH"] = ":".join(extra_paths) + ":" + env.get("PATH", "")
        gws_cmd = shutil.which("gws") or "gws"

        # gws gmail send
        result = subprocess.run(
            [
                gws_cmd, "gmail", "users", "messages", "send",
                "--params", json.dumps({
                    "userId": "me",
                    "resource": {
                        "raw": _encode_email(subject, body)
                    }
                }),
            ],
            capture_output=True, text=True, timeout=15, env=env,
        )
        if result.returncode == 0:
            log.info("Email sent via gws")
            return True
        else:
            log.warning(f"Email send failed: {result.stderr[:200]}")
            return False

    except Exception as e:
        log.warning(f"Email notification failed: {e}")
        return False


def _encode_email(subject: str, body: str) -> str:
    """Encode email as base64 MIME for Gmail API."""
    import base64
    from email.mime.text import MIMEText

    msg = MIMEText(body)
    msg["Subject"] = subject
    msg["To"] = "me"  # Send to self
    return base64.urlsafe_b64encode(msg.as_bytes()).decode("ascii")


# ── Main Loop ───────────────────────────────────────────

def check_once() -> int:
    """Check for new stories, notify, return count of new stories."""
    log.info("Checking Hacker News top stories...")

    stories = fetch_top_stories(TOP_K)
    log.info(f"Fetched {len(stories)} top stories")

    seen = load_seen()
    new_stories = find_new_stories(stories, seen)

    if not new_stories:
        log.info("No new stories above threshold")
        # Still mark all as seen
        seen = mark_seen(stories, seen)
        save_seen(seen)
        return 0

    log.info(f"Found {len(new_stories)} new stories!")

    # Format notification
    message = format_stories_message(new_stories)

    # Send to Telegram
    tg_ok = send_telegram(message)

    # Send email (plain text version)
    plain = message.replace("*", "").replace("🔶", ">>").replace("💬", "comments:")
    email_ok = send_email(f"HN: {len(new_stories)} new stories", plain)

    # Log to Brain journal
    try:
        from core.superbrain.superbrain import Superbrain
        sb = Superbrain()
        titles = ", ".join(s.get("title", "?")[:40] for s in new_stories[:3])
        sb.remember("hn_monitor", "hackernews", f"{len(new_stories)} new stories: {titles}")
    except Exception:
        pass

    # Mark all current stories as seen
    seen = mark_seen(stories, seen)
    save_seen(seen)

    status = []
    if tg_ok:
        status.append("telegram")
    if email_ok:
        status.append("email")
    log.info(f"Notified via: {', '.join(status) if status else 'none (check config)'}")

    return len(new_stories)


def main():
    if "--test" in sys.argv:
        # Test mode: fetch and send one notification regardless of seen state
        stories = fetch_top_stories(5)
        if stories:
            message = format_stories_message(stories[:3])
            print(f"Test message:\n{message}\n")
            tg = send_telegram(message)
            print(f"Telegram: {'sent' if tg else 'failed'}")
        return

    if "--daemon" in sys.argv:
        log.info(f"Starting HN monitor daemon (check every {CHECK_INTERVAL}s)")
        while True:
            try:
                check_once()
            except Exception as e:
                log.error(f"Check failed: {e}")
            time.sleep(CHECK_INTERVAL)
    else:
        count = check_once()
        if count:
            print(f"{count} new stories found and notified")
        else:
            print("No new stories")


if __name__ == "__main__":
    main()
