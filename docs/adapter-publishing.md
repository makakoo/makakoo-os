# Publishing your own adapter

This doc is for adapter **publishers** — people writing `adapter.toml` manifests for new external AI endpoints and signing them for distribution. If you just want to *use* adapters, read `docs/adapters.md` first.

---

## The 30-line minimal adapter

The simplest useful adapter is an OpenAI-compatible HTTP endpoint:

```toml
[adapter]
name            = "myprovider"
version         = "0.1.0"
manifest_schema = 1
description     = "My provider, OpenAI-compatible"

[compatibility]
bridge_version = "^2.0"
protocols      = ["openai-chat-v1"]

[transport]
kind     = "openai-compatible"
base_url = "https://api.myprovider.com/v1"

[auth]
scheme  = "bearer"
key_env = "MYPROVIDER_API_KEY"

[output]
format        = "openai-chat"
verdict_field = "choices.0.message.content"

[capabilities]
supports_roles = ["validator", "delegate"]

[install]
source_type = "git"
source      = "https://github.com/me/myprovider-adapter.git"
ref         = "v0.1.0"

[security]
requires_network = true
allowed_hosts    = ["api.myprovider.com"]
requires_env     = ["MYPROVIDER_API_KEY"]
sandbox_profile  = "network-io"
signed_by        = "me"
```

Validate it locally:

```bash
makakoo adapter install ./myprovider-adapter --allow-unsigned --skip-health-check
makakoo adapter info myprovider
```

## Non-OpenAI transports

### Subprocess — wrap a CLI

```toml
[transport]
kind    = "subprocess"
command = ["my-cli", "--message", "{prompt}"]
stdin   = false

[output]
format = "lope-verdict-block"  # your CLI prints ---VERDICT---…---END---
```

Makakoo substitutes `{prompt}` into argv at call time. When `stdin = true`, the prompt is piped instead; argv `{prompt}` tokens are still substituted literally.

### MCP over HTTP

```toml
[transport]
kind = "mcp-http"
url  = "https://my-mcp-server/mcp"

[output]
format        = "openai-chat"
verdict_field = "result.content.0.text"
```

Makakoo sends a single `tools/call` JSON-RPC request at `url` and reads the response. Multi-step MCP sessions are planned for v0.4.

### MCP over stdio

```toml
[transport]
kind    = "mcp-stdio"
command = ["my-mcp-binary"]
```

The binary must read a single line of JSON-RPC from stdin and emit the result on stdout. Phase B ships a minimal single-shot dispatcher.

## Output formats

| `format`              | When to use                                                                                          |
| --------------------- | ---------------------------------------------------------------------------------------------------- |
| `lope-verdict-block`  | Your endpoint emits `---VERDICT---\n…\n---END---` blocks. Preferred for lope validators.             |
| `openai-chat`         | Standard OpenAI chat completions response. Extracted via `verdict_field` dot-path, then verdict-block-parsed recursively. |
| `plain`               | Free-form text. Makakoo applies a PASS/FAIL/NEEDS_FIX keyword heuristic, defaults to PASS at 0.5 confidence. |
| `custom`              | A Python parser in `module:fn` form. Reserved for v0.4 — Phase C rejects with a readable error today. |

## Signing your manifest

### Generate a keypair

Makakoo uses pure-Ed25519 (no PKI). Generate locally:

```bash
# ed25519-keygen (any tool that emits raw 32-byte seed works)
head -c 32 /dev/urandom > secret.key
# Derive the public key — use `ssh-keygen` if you prefer PEM, else a Rust/Python helper
```

For a production-grade keypair, use `age-keygen -o mykey.txt` and convert, or a Rust helper that calls `ed25519_dalek::SigningKey`.

### Register your public key

Users trust your adapter by copying your `.pub` file to their trust dir:

```bash
# As the publisher, ship this file with your adapter:
echo "<base64-encoded-32-byte-public-key>" > mypublisher.pub

# Users install it:
mkdir -p ~/.makakoo/trust/keys
cp mypublisher.pub ~/.makakoo/trust/keys/mypublisher.pub
```

Set `security.signed_by = "mypublisher"` in your manifest so Makakoo looks up the right key.

### Sign the manifest

```python
# Minimal Python signer (not shipped with Makakoo — roll your own)
import base64
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
key = Ed25519PrivateKey.from_private_bytes(open("secret.key", "rb").read())
manifest_bytes = open("adapter.toml", "rb").read()
sig = key.sign(manifest_bytes)
open("adapter.toml.sig", "w").write(base64.b64encode(sig).decode())
```

Ship `adapter.toml.sig` alongside `adapter.toml`.

## Security rules (these are contract, not advice)

1. **Never** embed secrets in the manifest. `security.requires_env` lists env var **names**; users supply values at call time.
2. **Default-deny** network egress. List every host the adapter will contact in `security.allowed_hosts`. Empty list + `requires_network = true` is rejected at parse time.
3. **Pin install sources.** Use semver tags (`v1.2.3`) or 40-char commit SHAs; `makakoo` rejects bare branches at parse time.
4. **sha256-pin non-git sources.** Every `https-tarball` and `binary` install needs a 64-hex `install.sha256`.
5. **Match your sandbox to what you actually need.** An adapter that only reads stdin + writes stdout should be `isolated`. Don't ask for `network-io` if you don't need outbound HTTP.
6. **Re-trust on change.** Don't silently widen `capabilities.features` or `security.allowed_hosts` in a minor bump — Makakoo's update flow will reject the install and prompt the user anyway. Bump the major version when security surface widens.
7. **Signed manifests everywhere except local dev.** URL installs without a `.sig` require an explicit `--allow-unsigned` flag from the user.

## Submitting to `makakoo-adapters-core`

The core pack at `github.com/makakoo/adapters-core` ships curated, signed adapters installable via:

```bash
makakoo adapter install makakoo-adapters-core --pack
```

To submit:

1. Fork the repo.
2. Add your adapter under `adapters/<name>/` with `adapter.toml`, `adapter.toml.sig`, and a `README.md`.
3. If your signing key isn't in `trust/keys/`, add it.
4. Open a PR against `main`.
5. Maintainers review + verify signature + run the install-time health check in a clean sandbox.
6. On merge, users get your adapter in the next `makakoo adapter install makakoo-adapters-core --pack`.

## Testing your adapter

```bash
# Parse + validate the manifest
makakoo adapter info --json myprovider < /dev/null

# Run end-to-end against a real or stub endpoint
echo "test prompt" | makakoo adapter call myprovider --bundled

# Full doctor report
makakoo adapter doctor myprovider
```

Write your own parser unit test in Python if you need custom output parsing:

```python
from lope.makakoo_adapter import MakakooAdapterValidator
v = MakakooAdapterValidator(adapter_name="myprovider")
result = v.validate("test prompt", timeout=30)
print(result.verdict.status, result.verdict.confidence, result.verdict.rationale)
```

## Versioning discipline

- **Patch bump** — bug fixes, typo in description, no capability change.
- **Minor bump** — new feature (e.g. add `streaming` to `capabilities.features`) that doesn't widen the security surface. Users will hit the re-trust prompt — that's correct.
- **Major bump** — breaking change: removed a capability, changed `sandbox_profile`, widened `allowed_hosts`, rotated `signed_by`. Users MUST re-trust.
- **manifest_schema bump** — only when the host binary can't read the old format anymore. As of v0.3 the schema is frozen at `1`.

## See also

- `spec/ADAPTER_MANIFEST.md` — canonical schema v1 with field-by-field reference
- `docs/adapters.md` — user-facing install + troubleshooting
- `development/sprints/MAKAKOO-OS-V0.3-UNIVERSAL-BRIDGE/SPRINT.md` — the sprint that shipped this system
