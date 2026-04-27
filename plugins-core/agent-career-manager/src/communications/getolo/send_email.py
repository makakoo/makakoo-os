import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: jobs@getolo.com
Subject: Freelance Senior Backend Engineer - Ruby on Rails & Claims
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw getolo is looking for a Senior Backend Engineer - Claims with a focus on Ruby, Rails, Microservices. 

I am a Senior Software Architect specializing in high-performance Ruby on Rails applications and Agentic AI orchestrations. I build the underlying infrastructure that allows complex web systems to scale efficiently.

Recently, I:
1. Architected and maintained enterprise-scale Ruby microservices handling massive transaction volumes.
2. Built `google_drive_forge`—a fully autonomous MCP server for complex system interactions.
3. Maintained the `a2a-ruby` SDK for Google’s Agent-to-Agent protocol.

I have strong availability for freelance/B2B contracts right now. Are you open to a brief chat to see if my background aligns with your roadmap?

Best,
{SENDER_NAME}
jevvellabs.com/cv.html | GitHub: @traylinx"""

    raw_message = base64.urlsafe_b64encode(message_text.encode("utf-8")).decode("utf-8")
    
    payload = {
        "raw": raw_message
    }

    cmd = [
        GWS_BIN,
        "gmail",
        "users",
        "messages",
        "send",
        "--params", json.dumps({"userId": "me"}),
        "--json", json.dumps(payload),
        "--format", "json"
    ]

    result = subprocess.run(cmd, capture_output=True, text=True)
    print(result.stdout)
    if result.stderr:
        print("Error:", result.stderr)

if __name__ == "__main__":
    send_email()
