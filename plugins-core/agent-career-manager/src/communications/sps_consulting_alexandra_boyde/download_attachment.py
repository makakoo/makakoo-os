import json
import os
import shutil
import subprocess
import base64
import sys
import argparse

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
GWS_BIN = shutil.which("gws") or "gws"

def download_attachment(message_id: str, attachment_id: str, output_path: str):
    params = {
        "userId": "me",
        "messageId": message_id,
        "id": attachment_id
    }

    cmd = [
        GWS_BIN,
        "gmail", "users", "messages", "attachments", "get",
        "--params", json.dumps(params),
        "--format", "json"
    ]

    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print("Error fetching attachment:", result.stderr)
        sys.exit(1)

    data = json.loads(result.stdout)
    file_data = base64.urlsafe_b64decode(data['data'])

    with open(output_path, "wb") as f:
        f.write(file_data)

    print(f"Attachment saved to {output_path}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Download a Gmail attachment via gws")
    parser.add_argument("--message-id", required=True, help="Gmail message ID")
    parser.add_argument("--attachment-id", required=True, help="Gmail attachment ID")
    parser.add_argument("--output", required=True, help="Output file path")
    args = parser.parse_args()
    download_attachment(args.message_id, args.attachment_id, args.output)
