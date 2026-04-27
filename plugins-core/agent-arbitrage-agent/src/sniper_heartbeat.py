#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Sniper Watchdog (Self-Healing)
-----------------------------------
Checks if the `btc_5m_sniper.py --live` process is running.
If it is NOT running, it AUTOMATICALLY RESTARTS IT in the background,
then triggers a macOS popup to inform the user.

Designed to be run via cron every 5 minutes (was hourly, now more frequent).
"""
import subprocess
import os
import sys
from datetime import datetime

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
LOG_FILE = os.path.join(HARVEY_HOME, "data", "logs", "sniper_heartbeat.log")
PROCESS_NAME = "btc_5m_sniper.py --live"
PYTHON_BIN = "/usr/local/opt/python@3.11/bin/python3.11"
SNIPER_SCRIPT = os.path.join(HARVEY_HOME, "plugins-core", "btc-sniper", "btc_5m_sniper.py")
SNIPER_LOG = os.path.join(HARVEY_HOME, "data", "logs", "btc_5m_sniper.log")

def log_event(msg):
    os.makedirs(os.path.dirname(LOG_FILE), exist_ok=True)
    with open(LOG_FILE, 'a') as f:
        f.write(f"[{datetime.now().strftime('%Y-%m-%d %H:%M:%S')}] {msg}\n")
    print(msg)

def is_running():
    """Check if the sniper process is alive."""
    try:
        cmd = f"ps aux | grep '{PROCESS_NAME}' | grep -v grep"
        output = subprocess.check_output(cmd, shell=True).decode('utf-8').strip()
        return bool(output)
    except subprocess.CalledProcessError:
        return False
    except Exception as e:
        log_event(f"⚠️ Error checking process: {e}")
        return False

def restart_sniper():
    """Restart the sniper bot in the background using the correct Python."""
    log_event("🔄 AUTO-RESTARTING sniper bot...")
    try:
        # Use nohup with the EXPLICIT python3.11 binary
        cmd = (
            f"nohup {PYTHON_BIN} -u {SNIPER_SCRIPT} --live "
            f">> {SNIPER_LOG} 2>&1 &"
        )
        subprocess.Popen(cmd, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        log_event("   ✅ Sniper restarted successfully.")
        return True
    except Exception as e:
        log_event(f"   ❌ Failed to restart sniper: {e}")
        return False

def show_popup(restarted=False):
    """Show macOS native alert."""
    if restarted:
        msg = "⚠️ The BTC 5-Min Sniper bot had stopped.\\n\\nIt has been AUTOMATICALLY RESTARTED by the watchdog."
        title = "Harvey OS — Bot Auto-Restarted"
    else:
        msg = "🚨 URGENT: The BTC 5-Min Sniper bot is NOT running and could NOT be restarted!\\n\\nPlease check manually."
        title = "Harvey OS Alert"

    apple_script = f'''
    display dialog "{msg}" with title "{title}" buttons {{"OK"}} default button "OK" with icon caution
    '''
    try:
        subprocess.run(['osascript', '-e', apple_script], check=False)
        log_event("   Popup displayed to user.")
    except Exception as e:
        log_event(f"   ⚠️ Failed to display popup: {e}")

def check_and_heal():
    if is_running():
        log_event("✅ Sniper is running.")
        return

    log_event("🚨 ALERT: BTC Sniper is NOT running!")
    restarted = restart_sniper()
    show_popup(restarted=restarted)

if __name__ == '__main__':
    check_and_heal()
