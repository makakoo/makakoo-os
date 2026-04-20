import os
import shutil
import subprocess
import base64
import json

SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")
GMAIL_THREAD_ID = os.environ.get("GMAIL_THREAD_ID", "THREAD_ID_HERE")
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
GWS_BIN = shutil.which("gws") or "gws"
ATTACHMENT_PATH = os.environ.get(
    "SPS_CV_PATH",
    os.path.join(HARVEY_HOME, "career", "communications",
                 "sps_consulting_alexandra_boyde",
                 "CV_Sebastian_Schkudlara_SPS_Consulting_2026-03-11_updated.docx")
)

def send_reply():
    message_text = f"""To: projektbewerbung@sps-cs.de
Subject: Re: Ihre Bewerbung KI - Bahn
In-Reply-To: <791gtAyJ6FNZzs9jJfeUe3nDAXx10gsrLBcvnTLNh90@k47670.coveto.de>
References: <791gtAyJ6FNZzs9jJfeUe3nDAXx10gsrLBcvnTLNh90@k47670.coveto.de>
Content-Type: multipart/mixed; boundary="boundary-sps-reply"

--boundary-sps-reply
Content-Type: text/plain; charset=utf-8

Hallo Frau Boyde,

vielen Dank für die Rückmeldung und das Profil.

Ich bin auch für eine sozialversicherungspflichtige Festanstellung (Arbeitnehmerüberlassung/Angestellter) absolut offen, das ist für mich kein Ausschlusskriterium.

Bezüglich der Anforderungen habe ich das Profil entsprechend meiner tatsächlichen historischen Erfahrung angepasst (siehe Anhang).

Sowohl im Bereich "Context Optimization" als auch bei der "Semantic Agent Optimization" habe ich meine Erfahrungen aus meiner langjährigen Zeit als Head of Development bei ChainGO Tech (2019-2024) ergänzt. Dort habe ich bereits tiefgreifend KI-Modelle zur Datenextraktion optimiert und in unsere Workflows integriert, sodass die Anforderung von >3 Jahren problemlos und wahrheitsgemäß erfüllt ist.

Die Zertifizierung für Agile Methoden (CAL) liegt mir formell nicht vor, jedoch habe ich 7+ Jahre praktische Erfahrung in der Führung agiler Teams.

Das angepasste Profil finden Sie im Anhang. Wir können die Details gerne gleich in unserem Telefonat besprechen.

Viele Grüße,
{SENDER_NAME}

--boundary-sps-reply
Content-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document; name="CV_Sebastian_Schkudlara_SPS_Consulting_2026-03-11_updated.docx"
Content-Disposition: attachment; filename="CV_Sebastian_Schkudlara_SPS_Consulting_2026-03-11_updated.docx"
Content-Transfer-Encoding: base64

"""

    with open(ATTACHMENT_PATH, "rb") as f:
        file_data = base64.b64encode(f.read()).decode('utf-8')

    message_text += file_data + "\n--boundary-sps-reply--\n"

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
