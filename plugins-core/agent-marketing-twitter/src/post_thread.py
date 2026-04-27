#!/usr/bin/env python3
"""
post_thread.py — real X/Twitter thread poster for the marketing-twitter agent.

Uploads media via X API v1.1 media endpoint, posts the first tweet via v2
with media_ids attached, then chains reply tweets via v2 reply.in_reply_to_tweet_id.

HALLUCINATION-PROOF CONTRACT:
    Does: Parse a TWITTER_THREAD.md file, upload images, post the thread,
          return the list of tweet IDs and the thread URL.
    Does NOT: Generate content. Schedule posts. Handle DMs. Fix API errors.
    Requires: tweepy installed, and OAuth 1.0a credentials in env:
              X_API_KEY, X_API_SECRET, X_ACCESS_TOKEN, X_ACCESS_SECRET
    On failure: Prints exactly what went wrong and exits non-zero.
                Never silently proceeds. Never invents workarounds.
"""
from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path
from typing import List, Optional, Tuple

REQUIRED_ENV = ("X_API_KEY", "X_API_SECRET", "X_ACCESS_TOKEN", "X_ACCESS_SECRET")

# Dotenv style loader — checks multiple credential locations in priority order:
#   1. ~/MAKAKOO/.env  (primary, also ~/MAKAKOO/.env via compat symlink)
#   2. ~/.config/x-cli/.env  (x-cli native location, used by xitter skill)
# Also maps X_ACCESS_TOKEN_SECRET → X_ACCESS_SECRET so either name works.
_CRED_FILES = [
    lambda: Path(os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", ""))) / ".env",
    lambda: Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / ".env",
    lambda: Path.home() / ".config" / "x-cli" / ".env",
]

_VAR_ALIASES = {
    "X_ACCESS_TOKEN_SECRET": "X_ACCESS_SECRET",
}

def _load_harvey_env() -> None:
    for resolver in _CRED_FILES:
        env_file = resolver()
        if not env_file.exists():
            continue
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, _, value = line.partition("=")
            key = key.strip()
            value = value.strip().strip('"').strip("'")
            if not key:
                continue
            # Map x-cli var names to the names this script expects
            mapped = _VAR_ALIASES.get(key)
            if mapped and mapped not in os.environ:
                os.environ[mapped] = value
            if key not in os.environ:
                os.environ[key] = value


def preflight() -> int:
    """Check all prerequisites. Return 0 if ready to post, non-zero otherwise."""
    _load_harvey_env()
    ok = True
    errors: List[str] = []

    try:
        import tweepy  # noqa: F401
        print("✓ tweepy installed")
    except ImportError:
        errors.append("tweepy not installed — run: pip3 install tweepy")
        ok = False

    missing = [k for k in REQUIRED_ENV if not os.environ.get(k)]
    if missing:
        errors.append(
            f"missing env vars: {', '.join(missing)}\n"
            f"  Searched:\n"
            f"    ~/MAKAKOO/.env  (or $MAKAKOO_HOME/.env)\n"
            f"    ~/MAKAKOO/.env   (compat)\n"
            f"    ~/.config/x-cli/.env  (x-cli / xitter skill)\n"
            f"  Required vars (name as in any of those files):\n"
            f"    X_API_KEY, X_API_SECRET\n"
            f"    X_ACCESS_TOKEN, X_ACCESS_SECRET  (or X_ACCESS_TOKEN_SECRET)\n"
            f"  Get these from https://developer.x.com/en/portal/projects-and-apps\n"
            f"  App must have 'Read and Write' permission enabled."
        )
        ok = False
    else:
        print(f"✓ all 4 X env vars present ({', '.join(REQUIRED_ENV)})")

    if ok:
        try:
            import tweepy
            auth = tweepy.OAuth1UserHandler(
                os.environ["X_API_KEY"], os.environ["X_API_SECRET"],
                os.environ["X_ACCESS_TOKEN"], os.environ["X_ACCESS_SECRET"],
            )
            api = tweepy.API(auth)
            user = api.verify_credentials()
            print(f"✓ authenticated as @{user.screen_name}")
        except Exception as exc:
            errors.append(f"auth failed: {type(exc).__name__}: {exc}")
            ok = False

    if errors:
        print("PREFLIGHT FAILED:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1

    print("✓ preflight passed — ready to post")
    return 0


TWEET_HEADER_RE = re.compile(r"^\*\*Tweet\s+(\d+)[^*]*\*\*\s*$", re.MULTILINE)


def parse_thread(md_path: Path) -> List[Tuple[int, str]]:
    """Parse a TWITTER_THREAD.md into a list of (tweet_number, tweet_body).

    Format expected:
        **Tweet N (role)**

        <body lines...>

        ---

    Returns tweets in order found. Raises on format errors.
    """
    if not md_path.exists():
        raise FileNotFoundError(f"thread file not found: {md_path}")

    text = md_path.read_text(encoding="utf-8")
    matches = list(TWEET_HEADER_RE.finditer(text))
    if not matches:
        raise ValueError(
            f"no tweet headers found in {md_path} — expected '**Tweet N (role)**' format"
        )

    tweets: List[Tuple[int, str]] = []
    for i, m in enumerate(matches):
        start = m.end()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        body = text[start:end]
        # Strip trailing --- separator and whitespace
        body = re.sub(r"\n+---\s*\n*\s*$", "", body).strip()
        if not body:
            raise ValueError(f"tweet {m.group(1)} has empty body")
        n = int(m.group(1))
        tweets.append((n, body))

    # Validate char limits
    over = [(n, len(b)) for n, b in tweets if len(b) > 280]
    if over:
        raise ValueError(
            f"{len(over)} tweet(s) over 280 chars: "
            + ", ".join(f"#{n}={length}" for n, length in over)
        )

    return tweets


def resolve_media_for_tweet(
    tweet_num: int, images_dir: Optional[Path]
) -> List[Path]:
    """Find image files matching this tweet number.

    Convention: tweet_N.png, tweet_N.jpg, tweet_comic_N.png, or N.png under images_dir.
    Returns list of matching paths (X allows up to 4 images per tweet).
    """
    if images_dir is None or not images_dir.exists():
        return []
    matches: List[Path] = []
    for pattern in (f"tweet_{tweet_num}.*", f"tweet_comic_{tweet_num}.*", f"{tweet_num}.*"):
        for p in sorted(images_dir.glob(pattern)):
            if p.is_file() and p.suffix.lower() in (".png", ".jpg", ".jpeg", ".gif", ".webp"):
                matches.append(p)
    # Max 4 per tweet per X policy
    return matches[:4]


def post_thread(
    thread_path: Path,
    images_dir: Optional[Path] = None,
    dry_run: bool = False,
) -> int:
    """Post a thread to X. Returns 0 on success, non-zero on failure."""
    _load_harvey_env()

    # Parse first — fail fast on format errors before touching the network
    try:
        tweets = parse_thread(thread_path)
    except (FileNotFoundError, ValueError) as exc:
        print(f"PARSE ERROR: {exc}", file=sys.stderr)
        return 2

    print(f"Parsed {len(tweets)} tweets from {thread_path}")
    for n, body in tweets:
        imgs = resolve_media_for_tweet(n, images_dir)
        img_note = f" [+{len(imgs)} img]" if imgs else ""
        print(f"  tweet {n}: {len(body)}/280 chars{img_note}")

    if dry_run:
        print("DRY RUN — no network calls made")
        return 0

    # Preflight creds before touching anything
    missing = [k for k in REQUIRED_ENV if not os.environ.get(k)]
    if missing:
        print(
            f"MISSING ENV VARS: {', '.join(missing)}\n"
            f"  Run `preflight` subcommand for setup instructions.",
            file=sys.stderr,
        )
        return 3

    try:
        import tweepy
    except ImportError:
        print("tweepy not installed — run: pip3 install tweepy", file=sys.stderr)
        return 4

    # v1.1 API for media upload
    auth = tweepy.OAuth1UserHandler(
        os.environ["X_API_KEY"], os.environ["X_API_SECRET"],
        os.environ["X_ACCESS_TOKEN"], os.environ["X_ACCESS_SECRET"],
    )
    api_v1 = tweepy.API(auth)

    # v2 Client for posting tweets (v1.1 /statuses/update is deprecated)
    client_v2 = tweepy.Client(
        consumer_key=os.environ["X_API_KEY"],
        consumer_secret=os.environ["X_API_SECRET"],
        access_token=os.environ["X_ACCESS_TOKEN"],
        access_token_secret=os.environ["X_ACCESS_SECRET"],
    )

    try:
        me = api_v1.verify_credentials()
        print(f"Posting as @{me.screen_name}")
    except Exception as exc:
        print(f"AUTH FAILED: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 5

    tweet_ids: List[str] = []
    previous_id: Optional[str] = None

    for n, body in tweets:
        # Upload any images for this tweet via v1.1
        media_ids: List[str] = []
        for img_path in resolve_media_for_tweet(n, images_dir):
            try:
                media = api_v1.media_upload(str(img_path))
                media_ids.append(media.media_id_string)
                print(f"  uploaded {img_path.name} → media_id {media.media_id_string}")
            except Exception as exc:
                print(
                    f"MEDIA UPLOAD FAILED (tweet {n}, {img_path.name}): "
                    f"{type(exc).__name__}: {exc}",
                    file=sys.stderr,
                )
                return 6

        # Post tweet via v2
        kwargs = {"text": body}
        if media_ids:
            kwargs["media_ids"] = media_ids
        if previous_id is not None:
            kwargs["in_reply_to_tweet_id"] = previous_id

        try:
            resp = client_v2.create_tweet(**kwargs)
            tweet_id = str(resp.data["id"])
            tweet_ids.append(tweet_id)
            previous_id = tweet_id
            print(f"  ✓ tweet {n} posted → {tweet_id}")
        except Exception as exc:
            print(
                f"POST FAILED (tweet {n}): {type(exc).__name__}: {exc}\n"
                f"  Body was: {body[:80]}...\n"
                f"  Posted so far: {tweet_ids}",
                file=sys.stderr,
            )
            return 7

    first_id = tweet_ids[0]
    print(f"\n✓ thread posted ({len(tweet_ids)} tweets)")
    print(f"  First tweet: https://x.com/{me.screen_name}/status/{first_id}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p_pre = sub.add_parser("preflight", help="Check prerequisites without posting")
    p_pre.set_defaults(func=lambda _: preflight())

    p_parse = sub.add_parser("parse", help="Parse a thread file and print the tweets")
    p_parse.add_argument("--file", required=True, type=Path)
    p_parse.set_defaults(
        func=lambda args: (print_parsed(args.file), 0)[1]
    )

    p_post = sub.add_parser("post", help="Post a thread")
    p_post.add_argument("--file", required=True, type=Path)
    p_post.add_argument("--images", type=Path, default=None, help="Dir with tweet_N.{png,jpg}")
    p_post.add_argument("--dry-run", action="store_true")
    p_post.set_defaults(
        func=lambda args: post_thread(args.file, args.images, args.dry_run)
    )

    args = parser.parse_args()
    return args.func(args)


def print_parsed(path: Path) -> None:
    try:
        tweets = parse_thread(path)
    except Exception as exc:
        print(f"PARSE ERROR: {exc}", file=sys.stderr)
        sys.exit(2)
    for n, body in tweets:
        print(f"--- Tweet {n} ({len(body)}/280) ---")
        print(body)
        print()


if __name__ == "__main__":
    raise SystemExit(main())
