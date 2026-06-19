# Security Policy

We treat security reports as priority work. The full policy is mirrored at
[makakoo.com/security](https://makakoo.com/security/).

## Reporting a vulnerability

**Preferred:** open a private security advisory at
[github.com/makakoo/makakoo-os/security/advisories/new](https://github.com/makakoo/makakoo-os/security/advisories/new).

**Alternative:** email `sebastian.schkudlara@gmail.com` with subject prefix `[SECURITY]`.

Please **do not** open a public GitHub issue for security problems before they're fixed.

## What to include

- Affected version or commit hash
- Reproduction steps (smallest possible)
- Impact: what an attacker can do
- Suggested fix, if any

## Response

We aim to acknowledge reports within **72 hours** and to publish a fix or mitigation
as quickly as the severity warrants. We will credit you in the advisory unless you
request anonymity.

## Scope

In scope:

- The `makakoo` CLI and `makakoo-mcp` binary
- Bundled adapters and the kernel under this repository
- Install scripts served from `makakoo.com/install` and `makakoo.com/install.ps1`

Out of scope:

- Third-party plugins not vendored in this repo
- Your local LLM provider's API
- Netlify infrastructure (report to [Netlify](https://www.netlify.com/security/))

## Verifying installs

Every release publishes, alongside each tarball/zip at
[github.com/makakoo/makakoo-os/releases](https://github.com/makakoo/makakoo-os/releases):

- a per-artifact `<file>.sha256` sidecar, and
- a single combined `SHA256SUMS` manifest covering every artifact.

Verify a download against either one:

```sh
# one artifact against its sidecar
shasum -a 256 -c makakoo-aarch64-apple-darwin.tar.gz.sha256

# or everything at once against the manifest
shasum -a 256 -c SHA256SUMS        # sha256sum -c SHA256SUMS on Linux
```

The install scripts (`install.sh` / `install.ps1`) run this check
automatically before unpacking and **abort on mismatch**. Set
`MAKAKOO_SKIP_CHECKSUM=1` only if you are deliberately installing an
unpublished build.

Each release artifact also carries **Sigstore build provenance**, so you
can confirm it was built by this repo's CI from this source — not
re-packaged by someone in between:

```sh
gh attestation verify makakoo-aarch64-apple-darwin.tar.gz --repo makakoo/makakoo-os
```

The Homebrew formula at [traylinx/homebrew-tap](https://github.com/traylinx/homebrew-tap)
pins exact SHA-256 hashes per platform.
