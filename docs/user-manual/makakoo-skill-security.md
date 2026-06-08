# Skill Security — SkillSpector Gate and Audit

To protect your system from executing unsafe code, Makakoo OS integrates **NVIDIA SkillSpector** as a security gate. SkillSpector runs static analysis by default on plugin entrypoints and skill code, checking for credential access, suspicious network payloads, arbitrary execution, and other risk factors. Optional LLM semantic triage is available behind `--llm`, but is not the default gate.

This page explains how the preflight gate behaves, how to override risk blocks when installing trusted packages, and how to run manual audits on your plugin fleet.

---

## 1. Plugin Preflight Security Gate

When you run `makakoo plugin install <source>`, Makakoo automatically runs a **preflight security scan** on the target directory before completing the installation.

### Risk Policy and Severity Levels
SkillSpector assigns a numeric risk score (0 to 100) and a severity level to each target:

| Severity | Risk Score Range | Default Action |
|:---|:---|:---|
| **LOW** / **SAFE** | 0 – 39 | Install allowed silently |
| **MEDIUM** / **CAUTION** | 40 – 74 | Install allowed (with warning) |
| **HIGH** | 75 – 89 | **Blocked** (requires explicit override) |
| **CRITICAL** | 90 – 100 | **Blocked** (requires explicit override) |

If a plugin is flagged with a **HIGH** or **CRITICAL** severity (e.g. score >= 85), the installation is aborted with an error message:

```text
Error: SkillSpector flagged this plugin: HIGH 85/100
Install blocked pending explicit override.
```

---

## 2. Risk Overrides and False-Positives

If you are installing a custom or proprietary plugin, or if you have reviewed a flagged plugin's source code and verified that the warning is a false-positive, you can override the installation block.

### Override Syntax: `--allow-risk` and `--risk-ack`
To force installation of a blocked plugin, pass the `--allow-risk` flag and provide a non-empty explanation to `--risk-ack`:

```sh
makakoo plugin install git+https://github.com/acme/my-flagged-plugin@v1.0.0 \
  --allow-risk \
  --risk-ack "Reviewed source file main.py line 5: API credentials are from a mock environment."
```

> [!IMPORTANT]
> The `--risk-ack` explanation must be a non-empty string. Passing `--allow-risk` without `--risk-ack` will result in a CLI validation error.

When an override is applied:
1. The plugin is successfully installed to `$MAKAKOO_HOME/plugins/`.
2. The override configuration is recorded in the companion file at `$MAKAKOO_HOME/state/plugin-risk/<plugin-name>.json` with the following schema:
   ```json
   {
     "override": true,
     "override_ack": "Reviewed source file main.py line 5: API credentials are from a mock environment."
   }
   ```
3. Future updates to this plugin will prompt for a re-trust check if the manifest changes.

### Bypassing Scans on Local Paths: `--no-skill-scan`
For local development, you can skip the security scan entirely using the `--no-skill-scan` flag:

```sh
makakoo plugin install ./my-local-plugin --no-skill-scan
```

> [!WARNING]
> `--no-skill-scan` is strictly restricted to **local path installs**. Attempting to use `--no-skill-scan` on a remote source (such as `git+https://...`) will be blocked at CLI validation:
> `error: --no-skill-scan is only allowed for local path installs`

---

## 3. Manual Fleet Auditing (`makakoo skill audit`)

You can run security audits on demand to inspect files or scan your entire workspace.

### Audit one target
To run a security scan against a specific directory, file, or remote repository:

```sh
makakoo skill audit /path/to/my-plugin
```

### Audit the entire fleet (`--all`)
To scan installed plugins and local skill roots (`$MAKAKOO_HOME/plugins/`, `$MAKAKOO_HOME/skills-shared/`, `~/.agents/skills/`, `~/.codex/skills/`, `~/.claude/skills/`, `~/.lope/skills/`, plus the current workspace):

```sh
makakoo skill audit --all
```

#### Fleet Audit Exclusions
The fleet auditor automatically skips common build and dependency folders (vendor directories) to avoid noise and slow scans. Excluded folders include:
- `node_modules/`
- `target/`
- `.git/`

#### Limiting Scanned Targets
To run a quick sample scan, use the `--limit` option to cap the number of targets checked:

```sh
makakoo skill audit --all --limit 5
```

---

## 4. Reports and Output Formats

Every security scan generates machine-readable reports saved under `$MAKAKOO_HOME/data/reports/skillspector/`.

### Directory Structure
Reports are organized in dated directories:
```text
$MAKAKOO_HOME/data/reports/skillspector/
└── 2026-06-08/
    ├── my-plugin.json
    ├── my-plugin.sarif
    ├── fleet-summary.json
    └── fleet-summary.md
```

### Output Formats
By default, the CLI prints a structured human-readable table with risk levels, scores, and the top 10 findings. You can request other output formats:

* **JSON Format (`--json`):** Emits the complete report structure directly to `stdout`.
  ```sh
  makakoo skill audit /path/to/plugin --json
  ```
* **SARIF Format (`--sarif <file>`):** Emits standard SARIF logs compatible with CI environments and security dashboards (like GitHub Code Scanning).
  ```sh
  makakoo skill audit /path/to/plugin --sarif report.sarif
  ```

For fleet audits, a consolidated summary is written in both JSON (`fleet-summary.json`) and Markdown (`fleet-summary.md`) formats inside the daily report folder.

---

## 5. Configuration

Makakoo reads scanner policy from `$MAKAKOO_HOME/config/skill_security.toml`. If the file is missing, these defaults apply:

```toml
[skillspector]
enabled = true
mode = "static"
block_on = "high"
allow_override = true
pinned_git = "https://github.com/NVIDIA/SkillSpector"
pinned_ref = "939da7d41eed4282e4d8217fe2254c69f690027e"
report_dir = "$MAKAKOO_HOME/data/reports/skillspector"
```

Policy knobs:

- `enabled=false` disables plugin preflight scanning globally. Use sparingly.
- `block_on="medium"` blocks medium and above; `block_on="critical"` blocks only critical; `block_on="off"` reports without blocking.
- `allow_override=false` disables `--allow-risk` overrides.
- `report_dir` controls where JSON/SARIF/fleet reports are written.

---

## 6. macOS prerequisites

SkillSpector requires Python 3.12 and `uv`. Install both with Homebrew:

```sh
brew install python@3.12 uv
```

Makakoo searches these locations first:

- `/opt/homebrew/opt/python@3.12/bin/python3.12`
- `/usr/local/opt/python@3.12/bin/python3.12`
- `python3.12` on `$PATH`
- `~/.local/bin/uv`, `/opt/homebrew/bin/uv`, `/usr/local/bin/uv`, then `uv` on `$PATH`

The SkillSpector virtualenv lives under `$MAKAKOO_HOME/state/skillspector-venv/` and is rebuilt when the pinned SkillSpector ref changes or when you pass `--no-cache`.

---

## 7. Optional LLM semantic triage

Static mode is the shipping default for both `makakoo plugin install` and `makakoo skill audit`. To test SkillSpector's semantic mode manually, pass `--llm`:

```sh
makakoo skill audit /path/to/plugin --llm
```

For a Tytus/OpenAI-compatible gateway, export provider env vars before running the command:

```sh
export SKILLSPECTOR_PROVIDER=openai
export OPENAI_BASE_URL=http://10.42.42.1:18080/v1
export OPENAI_API_KEY=<stable-tytus-key>
export SKILLSPECTOR_MODEL=ail-compound
```

Local gateway variant:

```sh
export OPENAI_BASE_URL=http://127.0.0.1:18080/v1
```

Treat LLM triage as experimental: if gateway mode is slow or flaky, LLM triage must be considered experimental and left disabled. Never commit gateway keys or write them into docs, reports, or `skill_security.toml`.

---

## 8. False positives

SkillSpector is intentionally conservative. Expect noise from:

- tests that contain scary strings like `/etc/passwd`, fake tokens, or keychain fixture names;
- browser-cookie import helpers;
- install scripts that call shell commands;
- skills that legitimately wrap network or filesystem tools.

Use the JSON/SARIF report to inspect exact files and lines before overriding. The right posture is **warn + evidence + explicit acknowledgement**, not blind hard-blocking of known-local code.

