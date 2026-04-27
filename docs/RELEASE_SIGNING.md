# Release signing runbook

The hand-written release workflow at `.github/workflows/release.yml` ships
unsigned tarballs and zip archives. For v0.1 this is acceptable — users
install via `curl | sh` or `iwr | iex` and inspect the binary hash out of
band. For v0.2+ signed distribution is mandatory. This doc is the runbook
for flipping the workflow to the signed path once the certificates are in
hand.

**Budget:** ~$400 / yr (Apple Developer $99 + Authenticode cert $300).

## macOS — Apple notarization

Prerequisites:

1. Apple Developer Program enrollment ($99/yr) under the Makakoo legal
   entity (not a personal account — Sebastian's personal Apple ID should
   stay separate for legal hygiene).
2. A "Developer ID Application" certificate created via the portal. This
   is NOT the "Mac App Store" cert — notarization uses the Developer ID
   family.
3. An app-specific password for `xcrun notarytool` (generate at
   appleid.apple.com → App-Specific Passwords).

### GitHub Secrets to create

| Secret name                    | Value                                                    |
| ------------------------------ | -------------------------------------------------------- |
| `APPLE_DEVELOPER_ID_P12`       | Base64-encoded `.p12` export of the Developer ID cert    |
| `APPLE_DEVELOPER_ID_P12_PASS`  | Passphrase set when exporting the `.p12`                 |
| `APPLE_NOTARY_APPLE_ID`        | Apple ID used for notarization                           |
| `APPLE_NOTARY_PASSWORD`        | App-specific password from step 3                        |
| `APPLE_NOTARY_TEAM_ID`         | 10-character team ID from the Developer portal           |

### Workflow changes

Inside the `macos-latest` matrix jobs, after `Package tarball`:

```yaml
- name: Import signing certificate
  if: matrix.os == 'macos-latest'
  env:
    P12_BASE64: ${{ secrets.APPLE_DEVELOPER_ID_P12 }}
    P12_PASSWORD: ${{ secrets.APPLE_DEVELOPER_ID_P12_PASS }}
  run: |
    set -euo pipefail
    echo "$P12_BASE64" | base64 --decode > cert.p12
    security create-keychain -p build build.keychain
    security default-keychain -s build.keychain
    security unlock-keychain -p build build.keychain
    security import cert.p12 -k build.keychain -P "$P12_PASSWORD" \
      -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple:,codesign: \
      -s -k build build.keychain

- name: Sign + notarize
  if: matrix.os == 'macos-latest'
  env:
    NOTARY_ID: ${{ secrets.APPLE_NOTARY_APPLE_ID }}
    NOTARY_PASS: ${{ secrets.APPLE_NOTARY_PASSWORD }}
    NOTARY_TEAM: ${{ secrets.APPLE_NOTARY_TEAM_ID }}
  run: |
    set -euo pipefail
    stage="makakoo-${{ matrix.target }}"
    codesign --sign "Developer ID Application" --timestamp \
      --options runtime --force \
      "$stage/makakoo" "$stage/makakoo-mcp"
    # Re-bundle and notarize.
    tar -czf "$stage.tar.gz" "$stage"
    xcrun notarytool submit "$stage.tar.gz" \
      --apple-id "$NOTARY_ID" --password "$NOTARY_PASS" \
      --team-id "$NOTARY_TEAM" --wait
```

## Windows — Authenticode

Prerequisites:

1. An EV or OV Authenticode certificate (~$300/yr from Sectigo, DigiCert,
   or SSL.com). EV is a physical USB token; OV is a password-protected
   `.pfx`. For CI, OV is the pragmatic choice — EV's USB requirement
   blocks unattended CI signing without a KMS bridge.
2. SignTool from the Windows SDK (pre-installed on `windows-latest`).

### GitHub Secrets

| Secret name                 | Value                              |
| --------------------------- | ---------------------------------- |
| `WINDOWS_CODE_SIGNING_PFX`  | Base64-encoded `.pfx` cert         |
| `WINDOWS_CODE_SIGNING_PASS` | Passphrase for the `.pfx`          |

### Workflow changes

Inside the `windows-latest` matrix job, after `Package zip`:

```yaml
- name: Sign executables
  if: matrix.os == 'windows-latest'
  shell: pwsh
  env:
    PFX_BASE64: ${{ secrets.WINDOWS_CODE_SIGNING_PFX }}
    PFX_PASS:   ${{ secrets.WINDOWS_CODE_SIGNING_PASS }}
  run: |
    $pfxBytes = [Convert]::FromBase64String($env:PFX_BASE64)
    [IO.File]::WriteAllBytes("cert.pfx", $pfxBytes)
    $signtool = (Resolve-Path "$env:ProgramFiles(x86)\Windows Kits\10\bin\*\x64\signtool.exe" | Select -Last 1).Path
    & $signtool sign /f cert.pfx /p "$env:PFX_PASS" `
        /tr http://timestamp.digicert.com /td sha256 /fd sha256 `
        "target/${{ matrix.target }}/release/makakoo.exe" `
        "target/${{ matrix.target }}/release/makakoo-mcp.exe"
    # Re-pack zip with signed binaries.
    Remove-Item "makakoo-${{ matrix.target }}.zip" -Force
    # …re-run the packaging step here, or reorder Sign before Package.
```

## Homebrew tap

Create `github.com/makakoo/homebrew-makakoo` with a single `Formula/`
directory. On each release:

1. Compute SHAs of the three macOS + Linux tarballs.
2. Fill the `sha256` placeholders in `distribution/homebrew/makakoo.rb`.
3. Commit + push to the tap repo under `Formula/makakoo.rb`.

cargo-dist's `tap = "makakoo/homebrew-makakoo"` config automates this
step once we run `cargo dist init` with the full signing flow. Until
then, a small shell script can do it manually — see the "manual
release" section of the CHANGELOG.

## winget (Windows Package Manager)

Draft manifest at `distribution/winget/makakoo.yaml`. Submission process:

1. Fork `microsoft/winget-pkgs`.
2. Add `manifests/m/makakoo/makakoo/0.1.0/` with the three YAML files
   (version, installer, defaultLocale).
3. Open a PR targeting `master`. Automated validation runs; a human
   reviewer typically merges within 1-2 business days.

The `wingetcreate` CLI can automate this:

```pwsh
wingetcreate new \
  --urls https://github.com/makakoo/makakoo-os/releases/download/v0.1.0/makakoo-x86_64-pc-windows-msvc.zip
```

Paste the three YAML files it emits into
`distribution/winget/<version>/` for version control before submitting.
