#!/usr/bin/env python3
"""
LinkedIn Autopilot (Headless Pitch Sender)
Reads pending drafts from the career-manager CRM and physically sends them via Playwright.
"""
import os
import sys
import glob
import re
import time
from playwright.sync_api import sync_playwright

# Setup dynamic paths
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
DATA_DIR = os.path.join(HARVEY_HOME, "data", "linkedin-outreach")
STATE_FILE = os.path.join(DATA_DIR, "state.json")
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
CRM_DIR = os.path.join(HARVEY_HOME, "data", "career-manager", "communications")

# Import Logseq Brain (Sibling Skill)
try:
    sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "logseq-brain"))
    from logseq_bridge import LogseqBrain
except ImportError:
    LogseqBrain = None

def get_pending_drafts():
    """Scans the CRM directory for any HISTORY.md with Status: `Drafting` and a Target Profile."""
    drafts = []
    for history_file in glob.glob(os.path.join(CRM_DIR, "*", "HISTORY.md")):
        with open(history_file, 'r') as f:
            content = f.read()
            
        if "**Status:** `Drafting`" in content:
            # Extract target URL
            target_match = re.search(r'\*\*Target Profile:\*\*\s*(https://www\.linkedin\.com/in/[^\s]+)', content)
            if not target_match:
                continue
                
            # Extract Pitch Text
            pitch_match = re.search(r'\*?\*?Proposed Message:\*?\*?\s*```(?:text)?\n(.*?)\n```', content, re.DOTALL)
            if not pitch_match:
                continue
                
            company = os.path.basename(os.path.dirname(history_file)).replace("_", " ").title()
            
            drafts.append({
                "company": company,
                "file_path": history_file,
                "url": target_match.group(1).strip(),
                "pitch": pitch_match.group(1).strip(),
                "full_content": content
            })
    return drafts

def mark_as_sent(draft):
    """Updates the HISTORY.md to Status: Sent and syncs to Brain."""
    content = draft['full_content'].replace("**Status:** `Drafting`", "**Status:** `Sent`")
    with open(draft['file_path'], 'w') as f:
        f.write(content)
        
    print(f"✅ Updated CRM status for {draft['company']}.")
    
    # Sync to brain
    try:
        sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "career-manager"))
        from sync_to_brain import sync_company_to_brain
        
        # Extract contact to keep it intact in the Brain page
        contact_match = re.search(r'\*\*Contact:\*\*\s*(.+)', content)
        contact = contact_match.group(1).replace('`', '').strip() if contact_match else "Hiring Manager"
        
        print(f"  🧠 Syncing CRM state to Brain: Company - {draft['company']}")
        sync_company_to_brain(draft['company'], content, status="Sent", contact=contact)
    except ImportError as e:
        print(f"  ⚠️ Could not import sync_to_brain: {e}")

def mark_as_error(draft, error_msg):
    """Updates the HISTORY.md to Status: Error and syncs to Brain."""
    content = draft['full_content'].replace("**Status:** `Drafting`", "**Status:** `Error`")
    # Append error detail
    content += f"\n\n## ❌ AUTOMATION ERROR\n- **Details:** {error_msg}\n"
    
    with open(draft['file_path'], 'w') as f:
        f.write(content)
        
    print(f"❌ Updated CRM status to Error for {draft['company']}.")
    
    # Sync to brain
    try:
        sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "career-manager"))
        from sync_to_brain import sync_company_to_brain
        
        contact_match = re.search(r'\*\*Contact:\*\*\s*(.+)', content)
        contact = contact_match.group(1).replace('`', '').strip() if contact_match else "Hiring Manager"
        
        print(f"  🧠 Syncing CRM state to Brain: Company - {draft['company']} (Error)")
        sync_company_to_brain(draft['company'], content, status="Error", contact=contact)
    except ImportError as e:
        print(f"  ⚠️ Could not import sync_to_brain: {e}")

def send_connection_request(draft):
    """Uses Playwright to navigate to a profile and send a connection request with a note."""
    if not os.path.exists(STATE_FILE):
        print("❌ Error: state.json not found. You must run session_saver.py first to log in.")
        return False

    with sync_playwright() as p:
        # Run headless! The magic happens in the background.
        print(f"🤖 Booting headless browser for {draft['company']}...")
        browser = p.chromium.launch(headless=True) 
        context = browser.new_context(storage_state=STATE_FILE)
        page = context.new_page()
        
        try:
            print(f"   Navigating to: {draft['url']}")
            page.goto(draft['url'])
            time.sleep(3) # Human-like delay
            
            # --- The Automation Logic ---
            # LinkedIn UI changes frequently, so we look for buttons by text
            
            # Scroll to top to avoid sticky header issues
            page.evaluate("window.scrollTo(0, 0)")
            time.sleep(1)
            
            # Look for action buttons specifically in the profile header
            profile_header = page.locator(".ph5.pb5").first
            
            # Check if already connected
            message_btn = profile_header.get_by_role("button", name="Message").first
            if message_btn.is_visible():
                print(f"   ℹ️ Already connected with {draft['company']}. Sending direct message is not implemented in this bot.")
                return False

            # 1. Click "Connect" (It might be under the "More" button)
            connect_btn = profile_header.get_by_role("button", name="Connect").first # (It might be under the "More" button)
            connect_btn = profile_header.get_by_role("button", name="Connect").first
            
            if not connect_btn.is_visible():
                # Try clicking 'More' first
                print("   'Connect' hidden. Looking inside 'More' menu...")
                more_btn = profile_header.get_by_role("button", name="More").first
                if more_btn.is_visible():
                    more_btn.click()
                    time.sleep(1)
                    # Target the dropdown menu specifically
                    dropdown = page.locator(".artdeco-dropdown__content").filter(has_text="Connect").first
                    if dropdown.is_visible():
                        connect_btn = dropdown.get_by_text("Connect", exact=True).first
            
            if not connect_btn.is_visible():
                print(f"❌ Error: Could not find the Connect button for {draft['company']}. Already connected?")
                page.screenshot(path=os.path.join(HARVEY_HOME, "tmp", f"linkedin_error_{draft['company'].replace(' ', '_')}.png"))
                return False
                
            connect_btn.click()
            time.sleep(2)
            
            # 2. Click "Add a note"
            add_note_btn = page.get_by_role("button", name="Add a note").first
            if add_note_btn.is_visible():
                add_note_btn.click()
                time.sleep(1)
            else:
                print("❌ Error: Could not find 'Add a note' button in the modal. Taking screenshot...")
                page.screenshot(path=os.path.join(HARVEY_HOME, "tmp", f"linkedin_error_{draft['company'].replace(' ', '_')}.png"))
                return False
                
            # 3. Paste the Pitch
            print("   Typing pitch...")
            textbox = page.get_by_role("textbox")
            textbox.fill(draft['pitch'][:199])
            time.sleep(2)
            
            # 4. Click Send
            print("   Clicking Send...")
            page.screenshot(path=os.path.join(HARVEY_HOME, "tmp", f"linkedin_debug_{draft['company'].replace(' ', '_')}.png"))
            send_btn = page.get_by_role("button", name="Send").first
            send_btn.click()
            
            time.sleep(3) # Wait for network request to finish
            print(f"🚀 Success! Pitch sent to {draft['company']}.")
            return True
            
        except Exception as e:
            print(f"❌ Playwright Error processing {draft['company']}: {e}")
            return False
            
        finally:
            browser.close()

def main():
    print("========================================")
    print(" HARVEY OS: LinkedIn Autopilot          ")
    print("========================================")
    
    drafts = get_pending_drafts()
    if not drafts:
        print("✅ No pending drafts with valid LinkedIn URLs found in the CRM.")
        return
        
    print(f"Found {len(drafts)} drafts pending transmission.")
    
    for draft in drafts:
        print(f"\nProcessing: {draft['company']}")
        try:
            success = send_connection_request(draft)
            
            if success:
                # Add LinkedIn URL block to content before marking as sent
                draft['full_content'] += f"\n- **LinkedIn URL:** {draft['url']}\n"
                draft['full_content'] += f"- **Pitch:**\n  ```text\n  {draft['pitch']}\n  ```\n"
                mark_as_sent(draft)
                
                # Log to Brain
                if LogseqBrain:
                    brain = LogseqBrain()
                    brain.log_daily_action(
                        action_type="Outreach Autopilot",
                        entity=draft['company'],
                        details=[f"Successfully navigated to LinkedIn and sent connection request.", f"Target: {draft['url']}", f"Pitch:\n```\n{draft['pitch']}\n```"],
                        tags=["#CRM", "#autopilot", "#sent"]
                    )
            else:
                mark_as_error(draft, "Failed to find Connect button or Add a note button.")
                
        except Exception as e:
            mark_as_error(draft, f"Playwright Exception: {str(e)}")
                
        # Safety delay between leads to avoid bot detection
        time.sleep(5) 

if __name__ == "__main__":
    main()
