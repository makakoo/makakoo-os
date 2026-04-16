# makakoo-client (Python)

Python client for the Makakoo kernel capability socket. Mirrors the
shape of the Rust `makakoo-client` crate so a plugin written in Python
gets the same method surface and the same capability-enforcement
guarantees.

Pure stdlib — no external dependencies. Targets Python 3.9+.

## Usage

```python
from makakoo_client import Client, CapabilityDenied

client = Client.connect_from_env()

# state — backed by $MAKAKOO_HOME/state/<plugin>/
client.state_write("notes.txt", b"hello")
data = client.state_read("notes.txt")

# secrets — reads from the kernel's keyring via the socket
try:
    api_key = client.secret_read("AIL_API_KEY")
except CapabilityDenied as e:
    print(f"not allowed: {e}")
```

The plugin must declare the corresponding capability grants in its
`plugin.toml` — the kernel refuses any request that falls outside the
declared scope.

## Distribution

The package is vendored into each plugin's `.venv` at install time.
PyPI publishing is deferred to Phase I.
