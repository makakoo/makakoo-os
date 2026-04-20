import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: recruiting@taskifyai.com
Subject: Freelance Full Stack Engineer - AI Agent Infrastructure
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw Taskify AI is looking for a Full Stack Engineer with a focus on Python, Microservices, and AI Agent Infrastructure. 

I am a Senior Software Architect and Agentic AI Engineer with deep expertise in building Python microservices and multi-agent systems. I build the underlying infrastructure that allows AI tools to function autonomously at scale.

Recently, I:
1. Built a fully autonomous MCP server for complex system interactions.
2. Architected a routing proxy for managing multi-LLM traffic.
3. Deployed asynchronous Python (LangGraph) pipelines for agent orchestration.

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
