# Makakoo OS — distribution

Packaging config for shipping Makakoo OS binaries. The Rust workspace
at `../` builds two binaries — `makakoo` (umbrella CLI) and `makakoo-mcp`
(stdio MCP server) — and this directory describes how they're delivered
to users.

## Supported channels

| Channel | Install command |
|---|---|
| Homebrew (macOS + Linux) | `brew install makakoo/makakoo/makakoo` |
| Shell installer (macOS + Linux) | `curl -fsSL https://makakoo.com/install.sh \| sh` |
| Cargo (from source) | `cargo install --path makakoo` |

## Targets

| Triple | Status |
|---|---|
| `aarch64-apple-darwin` | primary |
| `x86_64-apple-darwin` | primary |
| `x86_64-unknown-linux-gnu` | best-effort |
| `aarch64-unknown-linux-gnu` | installer supports, not yet built |
| `x86_64-pc-windows-msvc` | future sprint |

## Layout

```
distribution/
├── README.md          # this file
├── install.sh         # curl | sh installer
└── homebrew/
    └── makakoo.rb     # Homebrew formula (SHAs filled at release time)
```

The cargo-dist packaging metadata lives in `../Cargo.toml` under
`[workspace.metadata.dist]`. CI generation is disabled (`ci = []`)
for this sprint — when infra lands, flip it to `ci = ["github"]` and
re-run `cargo dist init`.

## Releasing a new version

1. Bump `workspace.package.version` in `../Cargo.toml`.
2. `cargo dist build --artifacts=all` — local smoke build.
3. Tag `v<version>`, push tag.
4. Once infra exists, cargo-dist's GitHub Actions workflow builds
   per-target tarballs and uploads them to the GitHub Release.
5. Take the generated SHA256s and write them into
   `homebrew/makakoo.rb`, replacing the `PLACEHOLDER_*` strings.
6. Push the updated formula to the `makakoo/homebrew-makakoo` tap repo.

Until infra lands, steps 2–5 are run by hand on a dev machine and the
tarballs uploaded manually to the GitHub Release.

## Local validation

```bash
# Formula syntax (requires Homebrew)
brew style distribution/homebrew/makakoo.rb

# Installer sanity (requires shellcheck)
shellcheck distribution/install.sh

# cargo-dist config validation (requires cargo-dist)
cd .. && cargo dist check
```
