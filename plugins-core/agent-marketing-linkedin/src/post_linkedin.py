#!/usr/bin/env python3
"""
post_linkedin.py — LinkedIn UGC Posts API client for marketing-linkedin agent.

HALLUCINATION-PROOF CONTRACT:
    Does: Post a text update to LinkedIn via UGC Posts API. Preflight check.
    Does NOT: Upload images (v2 asset flow not implemented here — see
              https://learn.microsoft.com/en-us/linkedin/marketing/integrations/community-management/shares/vector-asset-api
              for the 2-step flow if you need it). Company pages. Polls.
              Comment posting. DMs. Analytics.
    Requires: LINKEDIN_ACCESS_TOKEN in env with `w_member_social` scope.
              Token acquired via OAuth 2.0 authorization code flow from a
              LinkedIn Developer App. Tokens expire in 60 days.
    On failure: Prints the exact LinkedIn API response body and exits
                non-zero. Never silently drops a post. Never claims the API
                "blocks" something without showing the real error.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import List

REQUIRED_ENV = ("LINKEDIN_ACCESS_TOKEN",)


# Mirror the search paths used by post_thread.py for Twitter
_CRED_FILES = [
    lambda: Path(os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", ""))) / ".env",
    lambda: Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / ".env",
    lambda: Path.home() / ".config" / "x-cli" / ".env",
]


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
            if key and key not in os.environ:
                os.environ[key] = value


def _api_get(url: str, token: str) -> dict:
    req = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "X-Restli-Protocol-Version": "2.0.0",
            "LinkedIn-Version": "202403",
        },
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def _api_post(url: str, token: str, body: dict) -> dict:
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-Restli-Protocol-Version": "2.0.0",
            "LinkedIn-Version": "202403",
        },
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return {
            "status": resp.status,
            "id": resp.headers.get("x-restli-id"),
            "body": json.loads(resp.read() or b"{}"),
        }


def preflight() -> int:
    _load_harvey_env()
    errors: List[str] = []

    missing = [k for k in REQUIRED_ENV if not os.environ.get(k)]
    if missing:
        errors.append(
            f"missing env vars: {', '.join(missing)}\n"
            f"  Add to ~/MAKAKOO/.env:\n"
            f"    LINKEDIN_ACCESS_TOKEN=<OAuth 2.0 user access token>\n"
            f"  Acquire via:\n"
            f"    1. Create LinkedIn app at https://www.linkedin.com/developers/apps\n"
            f"    2. Request 'Share on LinkedIn' product → grants w_member_social scope\n"
            f"    3. Run the OAuth 2.0 authorization code flow (3-legged)\n"
            f"    4. Exchange code for access_token at /oauth/v2/accessToken\n"
            f"    5. Token expires in 60 days — no refresh without partner status\n"
            f"  Full guide: https://learn.microsoft.com/en-us/linkedin/shared/authentication/authorization-code-flow"
        )

    if not errors:
        # Verify token works by calling /v2/userinfo
        token = os.environ["LINKEDIN_ACCESS_TOKEN"]
        try:
            me = _api_get("https://api.linkedin.com/v2/userinfo", token)
            name = me.get("name") or me.get("email") or me.get("sub", "unknown")
            print(f"✓ authenticated as: {name}")
            print(f"✓ person URN: urn:li:person:{me.get('sub', '?')}")
        except urllib.error.HTTPError as exc:
            body = exc.read().decode(errors="replace")
            errors.append(
                f"token auth failed: HTTP {exc.code}\n"
                f"  {body[:500]}\n"
                f"  Most common cause: token expired (LinkedIn tokens die after 60 days)\n"
                f"  Second cause: wrong scope — need w_member_social"
            )
        except Exception as exc:
            errors.append(f"auth check failed: {type(exc).__name__}: {exc}")

    if errors:
        print("PREFLIGHT FAILED:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1

    print("✓ preflight passed — ready to post text updates")
    return 0


def post_text(post_path: Path, dry_run: bool = False) -> int:
    _load_harvey_env()
    if not post_path.exists():
        print(f"post file not found: {post_path}", file=sys.stderr)
        return 2

    body_text = post_path.read_text(encoding="utf-8").strip()
    if not body_text:
        print(f"post file is empty: {post_path}", file=sys.stderr)
        return 2

    print(f"post length: {len(body_text)} chars")
    if len(body_text) > 3000:
        print(
            f"WARNING: LinkedIn UGC text limit is 3000 chars. "
            f"This post is {len(body_text)} — will be rejected.",
            file=sys.stderr,
        )
        return 3

    if dry_run:
        print("DRY RUN — no network calls")
        print(f"\nWould post:\n{body_text[:500]}...")
        return 0

    missing = [k for k in REQUIRED_ENV if not os.environ.get(k)]
    if missing:
        print(
            f"MISSING ENV: {', '.join(missing)} — run `preflight` for setup instructions",
            file=sys.stderr,
        )
        return 3

    token = os.environ["LINKEDIN_ACCESS_TOKEN"]

    # Get person URN
    try:
        me = _api_get("https://api.linkedin.com/v2/userinfo", token)
        person_urn = f"urn:li:person:{me['sub']}"
    except urllib.error.HTTPError as exc:
        body = exc.read().decode(errors="replace")
        print(f"AUTH FAILED: HTTP {exc.code}\n  {body[:500]}", file=sys.stderr)
        return 4

    # Build UGC post payload
    payload = {
        "author": person_urn,
        "lifecycleState": "PUBLISHED",
        "specificContent": {
            "com.linkedin.ugc.ShareContent": {
                "shareCommentary": {"text": body_text},
                "shareMediaCategory": "NONE",
            }
        },
        "visibility": {"com.linkedin.ugc.MemberNetworkVisibility": "PUBLIC"},
    }

    try:
        result = _api_post("https://api.linkedin.com/v2/ugcPosts", token, payload)
    except urllib.error.HTTPError as exc:
        body = exc.read().decode(errors="replace")
        print(
            f"POST FAILED: HTTP {exc.code}\n"
            f"  body: {body[:1000]}\n"
            f"  This is the raw LinkedIn API response — not a hallucinated explanation.",
            file=sys.stderr,
        )
        return 5
    except Exception as exc:
        print(f"POST FAILED: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 5

    post_id = result.get("id") or result["body"].get("id", "unknown")
    print(f"\n✓ posted: {post_id}")
    # LinkedIn share URL format
    urn_part = post_id.replace("urn:li:share:", "").replace("urn:li:ugcPost:", "")
    print(f"  URL: https://www.linkedin.com/feed/update/{post_id}/")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p_pre = sub.add_parser("preflight")
    p_pre.set_defaults(func=lambda _: preflight())

    p_post = sub.add_parser("post")
    p_post.add_argument("--file", required=True, type=Path, help="Path to post markdown file")
    p_post.add_argument("--dry-run", action="store_true")
    p_post.set_defaults(func=lambda args: post_text(args.file, args.dry_run))

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
