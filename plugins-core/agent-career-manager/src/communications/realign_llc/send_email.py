import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: careers@realign-llc.com
Subject: Freelance Python/MCP Engineer - Agentic AI Orchestration
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw Realign LLC is looking for a Python Engineer – Agentic AI & MCP Orchestration with a focus on Python, MCP Servers/Clients, LangGraph. 

I am a Senior Architect and Agentic AI Engineer specializing in Python, MCP, and high-throughput orchestration. I build the underlying infrastructure that allows AI agents to interface securely with local tools and production databases. 

Recently, I:
1. Built a fully autonomous MCP server for complex file interactions.
2. Architected a routing proxy for managing multi-LLM traffic at scale.
3. Deployed asynchronous Python (LangGraph) pipelines for agent orchestration.

I have strong availability for freelance/B2B contracts right now. Are you open to a brief chat to see if my background aligns with your roadmap?

Best,
{SENDER_NAME}
jevvellabs.com/cv.html"""

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
