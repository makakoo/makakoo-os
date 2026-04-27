import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: jobs@provenexpert.com
Subject: Freelance DevOps Teamlead - Infrastructure & Architecture
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw ProvenExpert is looking for a Teamlead DevOps Engineer, and given my 10+ years of infrastructure experience, I wanted to reach out directly.

I am a Senior Software Architect and DevOps Lead specializing in Kubernetes, AWS/GCP, and high-availability event-driven systems. I help teams stabilize their core infrastructure while transitioning to modern, agentic workflows. 

Recently, I:
1. Architected and managed large-scale Kubernetes clusters.
2. Built a fully autonomous routing proxy for high-throughput traffic.
3. Led teams to implement robust CI/CD and infrastructure-as-code (Terraform/Ansible) across complex microservice environments.

I am currently available for freelance/contract engagements and would love to bring this expertise to ProvenExpert. Are you open to a brief chat to see if my background aligns with your current roadmap?

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
