import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: careers@findr.ai
Subject: Freelance AI Engineer - Real Production AI
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw Findr is looking for an AI Engineer focusing on real production AI. Given your mission to automate search and workplace integrations, I wanted to reach out directly.

I am a Senior Software Architect and Agentic AI Engineer with deep expertise in Python, RAG, and multi-agent frameworks. I build the underlying systems that allow AI to safely interface with production databases and local APIs.

Recently, I:
1. Built a fully autonomous MCP (Model Context Protocol) server for complex file interactions.
2. Architected a routing proxy for managing multi-LLM traffic at scale.
3. Deployed asynchronous Python (LangGraph) pipelines for high-throughput AI orchestration.

I am currently available for freelance/B2B contracts. Are you open to a brief chat to see if my background aligns with your roadmap for production AI?

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
