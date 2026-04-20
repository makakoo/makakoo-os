import sys
import time
import json
import urllib.request
import urllib.error
from bs4 import BeautifulSoup
from playwright.sync_api import sync_playwright

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
STATE_FILE = os.path.join(HARVEY_HOME, "data", "linkedin-outreach", "state.json")

def verify_job_link(url):
    """
    Physically visits a job URL and checks for 'Death Signatures' 
    to ensure the job is actually still open.
    """
    death_signatures = [
        "no longer available",
        "job is closed",
        "position has been filled",
        "no longer accepting applications",
        "job not found",
        "page not found",
        "this role has been closed",
        "we are not currently hiring for this role"
    ]
    headers = {
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36'
    }
    try:
        req = urllib.request.Request(url, headers=headers)
        with urllib.request.urlopen(req, timeout=10) as response:
            if response.geturl() != url and response.geturl().rstrip('/') == url.split('/')[0] + "//" + url.split('/')[2]:
                 print(f"❌ DEAD (Redirected to homepage): {url}")
                 return False
            html = response.read().decode('utf-8', errors='ignore').lower()
            soup = BeautifulSoup(html, 'html.parser')
            text_content = soup.get_text()
            for sig in death_signatures:
                if sig in text_content:
                    print(f"❌ DEAD (Signature Found - '{sig}'): {url}")
                    return False
            print(f"✅ ALIVE & VERIFIED: {url}")
            return True
    except urllib.error.HTTPError as e:
        print(f"❌ DEAD (HTTP {e.code}): {url}")
        return False
    except Exception as e:
        print(f"⚠️ UNABLE TO VERIFY (Error: {e}): {url}")
        return False

def is_job_applied(url):
    print(f"🔍 Verifying job status: {url}")
    with sync_playwright() as p:
        try:
            browser = p.chromium.launch(headless=True)
            context = browser.new_context(storage_state=STATE_FILE)
            page = context.new_page()
            
            # Go to the job URL
            page.goto(url, timeout=15000)
            time.sleep(4) # Wait for DOM to settle
            
            # Check for "Applied" indicators
            if page.locator("text='Applied'").is_visible():
                return True
            if page.locator(".artdeco-inline-feedback--success").is_visible():
                return True
            apply_btn_text = page.locator(".jobs-apply-button--top-card button").text_content()
            if apply_btn_text and "Applied" in apply_btn_text:
                return True

        except Exception as e:
            print(f"⚠️ Warning: Could not verify {url}. Error: {e}")
        finally:
            if 'browser' in locals():
                browser.close()
            
    return False

if __name__ == "__main__":
    if len(sys.argv) > 1:
        url = sys.argv[1]
        print(f"Alive: {verify_job_link(url)}")
        print(f"Applied: {is_job_applied(url)}")
    else:
        print("Usage: python verify_job.py <job_url>")
