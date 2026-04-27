#!/usr/bin/env python3
"""
LinkedIn Session Saver
Run this script ONCE to log into LinkedIn manually.
It will save your authenticated session to a local JSON file so the main bot can run headlessly without logging in again.
"""
import os
import sys
from playwright.sync_api import sync_playwright

# Data Directory
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
DATA_DIR = os.path.join(HARVEY_HOME, "data", "linkedin-outreach")
os.makedirs(DATA_DIR, exist_ok=True)
STATE_FILE = os.path.join(DATA_DIR, "state.json")

def save_session():
    with sync_playwright() as p:
        # We use Chromium and open it visibly so the user can type their password / 2FA
        browser = p.chromium.launch(headless=False)
        context = browser.new_context()
        page = context.new_page()

        print("🌐 Opening LinkedIn...")
        page.goto("https://www.linkedin.com/login")
        
        print("\n" + "="*50)
        print("ACTION REQUIRED: Log into LinkedIn in the open browser window.")
        print("Wait until you are fully logged in and see your feed.")
        print("="*50 + "\n")

        # Wait until the user reaches the feed
        try:
            page.wait_for_url("https://www.linkedin.com/feed/", timeout=120000) # Wait up to 2 mins
            print("✅ Login detected! Saving session state...")
            
            # Save the session cookies to our data folder
            context.storage_state(path=STATE_FILE)
            print(f"🔒 Session securely saved to: {STATE_FILE}")
            
        except Exception as e:
            print(f"❌ Timed out or failed to reach the feed. Ensure you log in successfully.")
            
        finally:
            browser.close()

if __name__ == "__main__":
    save_session()
