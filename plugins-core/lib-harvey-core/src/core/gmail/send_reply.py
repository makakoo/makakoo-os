#!/usr/bin/env python3
"""
Gmail threaded reply sender.
Usage: python3 send_reply.py --to email@addr --subject "Re: ..." --reply-to-msg-id <gmail_msg_id> --body "message body"
"""
import base64, json, subprocess, sys, argparse
from email.mime.text import MIMEText


def get_message_headers(msg_id):
    """Fetch Message-ID, References, threadId from a Gmail message."""
    result = subprocess.run(
        ['gws', 'gmail', 'users', 'messages', 'get',
         '--params', json.dumps({"userId": "me", "id": msg_id, "format": "full"}),
         '--format', 'json'],
        capture_output=True, text=True, timeout=30
    )
    msg = json.loads(result.stdout)
    headers = {h['name'].lower(): h['value'] for h in msg['payload']['headers']}
    return {
        'message_id': headers.get('message-id', ''),
        'references': headers.get('references', ''),
        'thread_id': msg.get('threadId', ''),
    }


def send_reply(to, subject, body, reply_to_msg_id):
    """Send a threaded reply to an existing Gmail conversation."""
    info = get_message_headers(reply_to_msg_id)

    msg = MIMEText(body)
    msg['to'] = to
    msg['subject'] = subject
    msg['In-Reply-To'] = info['message_id']

    # Build References chain
    refs = info['references'] + ' ' + info['message_id'] if info['references'] else info['message_id']
    msg['References'] = refs.strip()

    raw = base64.urlsafe_b64encode(msg.as_bytes()).decode()
    payload = json.dumps({'raw': raw, 'threadId': info['thread_id']})

    result = subprocess.run(
        ['gws', 'gmail', 'users', 'messages', 'send',
         '--params', json.dumps({"userId": "me"}),
         '--json', payload],
        capture_output=True, text=True, timeout=30
    )
    resp = json.loads(result.stdout)
    return resp


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Send a threaded Gmail reply')
    parser.add_argument('--to', required=True, help='Recipient email')
    parser.add_argument('--subject', required=True, help='Email subject')
    parser.add_argument('--body', required=True, help='Email body text')
    parser.add_argument('--reply-to-msg-id', required=True, help='Gmail message ID to reply to')
    args = parser.parse_args()

    result = send_reply(args.to, args.subject, args.body, args.reply_to_msg_id)
    print(json.dumps(result, indent=2))
