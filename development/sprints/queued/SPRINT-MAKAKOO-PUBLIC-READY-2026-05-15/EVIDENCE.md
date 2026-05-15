# Evidence snapshot — 2026-05-15

## Repo and release
```
/Users/sebastian/makakoo-os
origin	https://github.com/makakoo/makakoo-os.git (fetch)
origin	https://github.com/makakoo/makakoo-os.git (push)
## main...origin/main
?? development/sprints/queued/SPRINT-MAKAKOO-PUBLIC-READY-2026-05-15/
2266d4b (HEAD -> main, origin/main) docs: sweep installer docs for v0.1.5 wizard auto-handoff
{"isPrerelease":false,"publishedAt":"2026-05-08T06:42:43Z","tagName":"v0.1.5","url":"https://github.com/makakoo/makakoo-os/releases/tag/v0.1.5"}
```

## File existence checks
```
EXISTS .github/workflows/ci.yml
EXISTS .github/workflows/smoke-public.yml
EXISTS .github/workflows/verify-docs.yml
EXISTS .github/workflows/release.yml
EXISTS ci/verify-docs.sh
EXISTS ci/run_on_clean.sh
EXISTS ci/docs_manifest.toml
EXISTS makakoo/tests/adapter_cli.rs
EXISTS makakoo/src/commands/install.rs
EXISTS makakoo/src/commands/distro.rs
EXISTS makakoo-core/src/platform.rs
EXISTS makakoo-core/src/superbrain/recall.rs
EXISTS makakoo-core/src/superbrain/store.rs
EXISTS makakoo-core/src/swarm/artifacts.rs
EXISTS makakoo-core/src/adapter/install.rs
EXISTS plugins-core/agent-browser-harness/install.sh
EXISTS plugins-core/agent-browser-harness/plugin.toml
EXISTS plugins-core/lib-harvey-core/bin/makakoo-venv-bootstrap
EXISTS distros/core.toml
EXISTS distribution/install.sh
EXISTS install/install.ps1
EXISTS README.md
EXISTS docs/getting-started.md
```

## Current failure evidence
```
CI latest main failure: https://github.com/makakoo/makakoo-os/actions/runs/25544289474
verify-docs latest main failure: https://github.com/makakoo/makakoo-os/actions/runs/25544289499
post-public smoke false-green run: https://github.com/makakoo/makakoo-os/actions/runs/25922943010
true isolated install failure: agent-browser-harness install.sh line 56 makakoo-venv-bootstrap command not found
homepage broken source link: https://github.com/traylinx/makakoo-os does not resolve
correct source link: https://github.com/makakoo/makakoo-os
Windows ARM64 release asset missing: makakoo-aarch64-pc-windows-msvc.zip
```

## Homepage/source location unknown
The deployed website content is live at https://makakoo.com/ but local source file was not found in /Users/sebastian/makakoo-os during sprint creation. Phase 4 starts with locating the website source/deploy repo.
