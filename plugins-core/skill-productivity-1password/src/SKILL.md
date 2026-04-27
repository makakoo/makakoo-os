# System Instruction: 1Password Integration

## Overview
This skill teaches Gemini CLI how to fetch secure credentials (like API keys) dynamically using the 1Password CLI (`op`).

**Mandate:** You must NEVER store plaintext API keys, passwords, or tokens in `.env` files or hardcoded scripts if they can be fetched securely from 1Password.

## Requirements
- The `op` (1Password) CLI must be installed and authenticated on the host machine.
- The user must be signed in: `eval "$(op signin)"`.

## How to Fetch Secrets
Use the `run_shell_command` tool to execute `op read` when you need a credential.

### Example: Fetching a Gemini API Key
If you need to use the Gemini API key in a script or a command, fetch it like this:
```bash
GEMINI_API_KEY=$(op read "op://Private/Gemini/credential")
```

### Passing Secrets to Python Scripts
If you are writing a script that needs a secret, do not hardcode it. Either:
1. Have the wrapper bash script fetch it and pass it as an environment variable:
   ```bash
   export GEMINI_API_KEY=$(op read "op://Private/Gemini/credential")
   python3 my_script.py
   ```
2. Or use Python's `subprocess` to fetch it at runtime:
   ```python
   import subprocess
   api_key = subprocess.check_output(['op', 'read', 'op://Private/Gemini/credential']).decode('utf-8').strip()
   ```

By using this skill, Harvey OS remains entirely stateless and secure.