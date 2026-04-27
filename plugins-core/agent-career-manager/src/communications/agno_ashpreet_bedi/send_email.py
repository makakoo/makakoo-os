import os
import base64
import json
import subprocess
from email.message import EmailMessage

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
PERSONAL_EMAIL = os.environ.get("PERSONAL_EMAIL", "your@email.com")

msg = EmailMessage()
msg.set_content(f"""Hi Ashpreet,

I've been following the rebrand to Agno and I'm a big fan of the "pure Python, no chains" philosophy. I saw you are looking for an EU-based Python Expert to help develop the Agno Agentic Framework, and my background is a 1:1 match.

As a Senior Freelance Architect based in Germany/Spain, I specialize in building the low-level infrastructure that powers autonomous agents:
- Protocol Level: I am the creator and maintainer of the `a2a-ruby` SDK for the Agent-to-Agent protocol.
- P2P Agent Mesh: I built Traylinx Stargate, a P2P networking layer for agent-to-agent communication using NATS/libp2p.
- MCP Ecosystem: I develop and maintain standalone MCP servers (e.g., `google_drive_forge`), and built `2md.traylinx.com` for AI-ready data ingestion.

I'm available for the 40h/week contract and can hit the ground running to help you expand Agno's tool integrations, memory optimizations, and core runtime capabilities.

Would you be open to a quick technical sync to discuss the framework's immediate roadmap?

Best,
{SENDER_NAME}
jevvellabs.com/cv.html | GitHub: @traylinx
""")

msg['Subject'] = 'Python Expert for Agno | Freelance Agentic AI Architect'
msg['From'] = PERSONAL_EMAIL
msg['To'] = 'ashpreet@agno.com, ashpreet@phidata.com'

raw_message = base64.urlsafe_b64encode(msg.as_bytes()).decode()

payload = json.dumps({"raw": raw_message})

cmd = ["gws", "gmail", "users", "messages", "send", "--params", '{"userId": "me"}', "--json", payload]
result = subprocess.run(cmd, capture_output=True, text=True)

print(result.stdout)
if result.stderr:
    print("Error:", result.stderr)
