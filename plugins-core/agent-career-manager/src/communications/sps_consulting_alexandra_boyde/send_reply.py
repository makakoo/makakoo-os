import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
PERSONAL_PHONE = os.environ.get("PERSONAL_PHONE", "[PHONE]")
GMAIL_THREAD_ID = os.environ.get("GMAIL_THREAD_ID", "THREAD_ID_HERE")
GWS_BIN = shutil.which("gws") or "gws"

def send_reply():
    message_text = f"""To: projektbewerbung@sps-cs.de
Subject: Re: Ihre Bewerbung
In-Reply-To: <9qXCGpy9OBSHJGhMP5RVEM5AokAPfry8Stuvy7idT8@k47670.coveto.de>
References: <9qXCGpy9OBSHJGhMP5RVEM5AokAPfry8Stuvy7idT8@k47670.coveto.de>
Content-Type: text/plain; charset=utf-8

Hallo Frau Boyde,

vielen Dank für die Rückmeldung. Ja, ich bin aktuell verfügbar und an dem Projekt interessiert.

Wir können sehr gerne morgen (Mittwoch) zwischen 09:30 und 10:30 Uhr telefonieren. Sie erreichen mich am besten unter der {PERSONAL_PHONE}.

Ich freue mich auf unseren Austausch!

Viele Grüße,
{SENDER_NAME}"""

    raw_message = base64.urlsafe_b64encode(message_text.encode("utf-8")).decode("utf-8")

    payload = {
        "raw": raw_message,
        "threadId": GMAIL_THREAD_ID
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
    send_reply()
