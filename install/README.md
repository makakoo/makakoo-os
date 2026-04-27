# install/

One-liner installers for Makakoo OS.

## Quick start

**macOS + Linux:**
```bash
curl -sSL https://makakoo.com/install | sh
```

**Windows (PowerShell):**
```powershell
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

Both scripts place the `makakoo` binary on `$PATH` and print a next
step that runs `makakoo install` to set up the kernel.

## What the scripts do

1. Detect OS + arch → resolve the matching cargo-dist release artifact
2. Download the tarball (macOS / Linux) or zip (Windows) from GitHub releases
3. Extract the `makakoo` binary into the install dir
4. Print a PATH hint if the install dir isn't already on `$PATH`
5. Print the next step (`makakoo install`)

The scripts **do not** run `makakoo install` themselves — that step
installs the core distro, registers the daemon, and infects AI CLI
hosts. Doing it automatically from a `curl | sh` invocation is more
magic than a user typically wants. Users can opt in by running
`makakoo install` manually after the binary is in place.

## Flags

Both scripts accept the same three options (with PowerShell vs. POSIX
conventions):

| Flag | Bash form | PowerShell form | Default |
|------|-----------|-----------------|---------|
| Pick a version | `--version 0.1.0` | `-Version 0.1.0` | `latest` |
| Install into DIR | `--install-dir DIR` | `-InstallDir DIR` | `~/.local/bin` (Unix) / `%LOCALAPPDATA%\Makakoo\bin` (Windows) |
| Dry-run | `--dry-run` | `-DryRun` | off |

Env-var overrides: `MAKAKOO_VERSION`, `MAKAKOO_INSTALL_DIR`, `MAKAKOO_REPO`,
`MAKAKOO_LOCAL_TARBALL` (for testing — skip download, use a local tarball).

## Windows Developer Mode

`install.ps1` refuses to run without Windows Developer Mode enabled.
Makakoo's infect step creates symlinks from AI CLI config directories
into `%LOCALAPPDATA%\Makakoo`, and unprivileged symlink creation on
Windows requires Developer Mode.

To enable: **Settings → Privacy & security → For developers → Developer Mode**.

## Testing the scripts locally

Both scripts support `MAKAKOO_LOCAL_TARBALL` for integration testing
without hitting GitHub:

```bash
# Build the binary
cargo build --release -p makakoo

# Pack it the way cargo-dist would
mkdir -p /tmp/mk && cp target/release/makakoo /tmp/mk/
tar -czf /tmp/mk.tar.gz -C /tmp/mk makakoo

# Run the installer against the local tarball
MAKAKOO_LOCAL_TARBALL=/tmp/mk.tar.gz \
MAKAKOO_INSTALL_DIR=/tmp/mk/bin \
bash install/install.sh
```

The Rust integration test at `makakoo/tests/install_sh.rs` exercises
this path automatically on every `cargo test`.

## Release artifact contract

Both scripts expect the release tarball / zip to contain `makakoo`
(`makakoo.exe` on Windows) at the archive root or one directory down.
This matches the layout `cargo-dist` produces with `installers = ["shell", "homebrew"]`
(Unix) and the analogous zip for Windows.

When `cargo-dist init ci = ["github"]` is flipped on in Phase G, the
GitHub Actions workflow it generates will upload artifacts matching
these filenames:

- `makakoo-aarch64-apple-darwin.tar.gz`
- `makakoo-x86_64-apple-darwin.tar.gz`
- `makakoo-x86_64-unknown-linux-gnu.tar.gz`
- `makakoo-aarch64-unknown-linux-gnu.tar.gz`
- `makakoo-x86_64-pc-windows-msvc.zip`
- `makakoo-aarch64-pc-windows-msvc.zip`

## Host of the one-liner URL

`https://makakoo.com/install` and `https://makakoo.com/install.ps1`
are 301 redirects to the raw files in this directory on GitHub. Wiring
up `makakoo.com` happens in Phase I alongside the public v0.1 launch.
Until then, use the direct GitHub URL:

```bash
curl -sSL https://raw.githubusercontent.com/makakoo/makakoo-os/main/install/install.sh | sh
```
