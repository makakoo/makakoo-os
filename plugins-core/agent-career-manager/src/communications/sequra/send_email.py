import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GWS_BIN = shutil.which("gws") or "gws"

def send_email():
    message_text = f"""To: jobs@sequra.es
Subject: Freelance Senior Ruby on Rails Engineer - Backend Architecture
Content-Type: text/plain; charset=utf-8

Hi Team,

I saw via Joppy that SeQura is looking for a Senior Backend Engineer specializing in Ruby on Rails. Given your focus on scalable financial infrastructure, I wanted to reach out directly.

I am a Senior Software Architect with over 15 years of experience building and scaling Ruby on Rails applications. Beyond standard web development, I specialize in complex integrations, high-availability event-driven systems (Kubernetes/Docker), and AI orchestrations. 

Recently, I:
1. Architected and maintained enterprise-scale Ruby microservices handling massive transaction volumes.
2. Maintained the `a2a-ruby` SDK for Google’s Agent-to-Agent protocol, showcasing deep expertise in advanced Ruby capabilities.
3. Transitioned monolithic structures into performant, scalable distributed systems.

I am currently available for B2B/freelance contracts. Are you open to a brief technical chat to see if my background aligns with your engineering roadmap for the upcoming quarter?

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
