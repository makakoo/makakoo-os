# `makakoo secret` — CLI reference

`makakoo secret` stores and retrieves API keys and credentials in the OS
keyring — macOS Keychain, Linux Secret Service (via `libsecret`), or
Windows Credential Manager. Secrets stored here never touch disk as
plaintext and are not visible in shell history. They are referenced in
adapter manifests and agent configs via `secret_ref = "KEY_NAME"` so the
daemon can read them directly without needing the env var to be exported.

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `secret set <KEY>` | Read a value from stdin (never echoed) and store it under `KEY`. |
| `secret get <KEY>` | Retrieve and print a secret. Reads a same-named env var first, then the OS keyring when an interactive prompt is safe. |
| `secret delete <KEY>` | Remove a stored secret from the keyring. |

Note: there is no `secret list` subcommand — the OS keyring does not expose
enumeration in the current implementation.

## Key use patterns

### Store an API key (value read from stdin)

```sh
# prompts silently for the value — not echoed, not in shell history
makakoo secret set AIL_API_KEY
# Enter secret: ████████████████

# verify the key was stored
makakoo secret get AIL_API_KEY
```

### Reference the secret in an adapter manifest

```toml
# adapter.toml
[auth]
secret_ref = "AIL_API_KEY"
# The daemon resolves this via the keyring at call time — no env var needed.
```

### Use from automation without GUI prompt storms

`makakoo secret get <KEY>` checks the environment variable `<KEY>` first. If
the env var is set and non-empty, Makakoo prints that value and never touches
the OS keyring.

When no env value exists, background/non-interactive callers fail fast instead
of opening macOS Keychain / desktop keyring prompts. This protects agent shells,
CI jobs, and daemon hooks from spawning repeated GUI password dialogs.

```sh
# Preferred in automation:
export AIL_API_KEY=...
makakoo secret get AIL_API_KEY

# Only for automation that explicitly accepts an OS GUI prompt:
MAKAKOO_SECRET_ALLOW_KEYCHAIN_PROMPT=1 makakoo secret get AIL_API_KEY
```

### Rotate a key

```sh
# overwrite: set accepts the same key name again
makakoo secret set AIL_API_KEY
# Enter secret: (new value)
```

### Remove a key that is no longer needed

```sh
makakoo secret delete OLD_PROVIDER_API_KEY
```

## Security notes

- The value is read via a non-echoing stdin prompt — it never appears in
  your terminal scroll-back or shell history.
- `secret get <KEY>` reads env `<KEY>` before the keyring. Non-interactive
  callers must either provide that env var or explicitly set
  `MAKAKOO_SECRET_ALLOW_KEYCHAIN_PROMPT=1`.
- The keyring service (Keychain / Secret Service / Credential Manager) is
  unlocked by your OS session. Keys are not accessible while the machine is
  locked or in a headless SSH session without the keyring daemon running.
- Use `secret_ref` in manifests and TOML configs instead of `inline_secret_dev`
  (which logs a `WARN` and is development-only).

## Related commands

- [`makakoo-adapter.md`](makakoo-adapter.md) — adapters use `secret_ref` to read keyring entries
- [`agent.md`](agent.md) — agent transports use the same secret resolution chain
- [`../concepts/architecture.md`](../concepts/architecture.md) — three-layer secret resolution

## Common gotcha

**`makakoo secret get <KEY>` exits with "secret not found" on Linux in a
headless/SSH session.**
The Linux Secret Service requires the GNOME keyring daemon or KWallet to be
running and unlocked. In headless environments (Docker, CI, SSH without agent
forwarding) neither is available. Work-around: export the key as an env var
in the service's environment instead, and reference it via `secret_env` in
the adapter manifest rather than `secret_ref`. For CI specifically, inject
the key via your CI platform's secrets mechanism and `export` it in the job
environment.
