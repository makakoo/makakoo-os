# Best-effort Windows installer for Headroom MCP.
$ErrorActionPreference = "Continue"
$proxyUrl = if ($env:HEADROOM_PROXY_URL) { $env:HEADROOM_PROXY_URL } else { "http://127.0.0.1:8787" }

function Find-Python310 {
  foreach ($candidate in @("py -3.12", "py -3.11", "py -3.10", "python")) {
    $parts = $candidate.Split(" ")
    $cmd = $parts[0]
    $args = @()
    if ($parts.Length -gt 1) { $args = $parts[1..($parts.Length - 1)] }
    try {
      & $cmd @args -c "import sys; raise SystemExit(0 if sys.version_info >= (3,10) else 1)" *> $null
      if ($LASTEXITCODE -eq 0) { return @($cmd) + $args }
    } catch {}
  }
  return $null
}

if (-not (Get-Command headroom -ErrorAction SilentlyContinue)) {
  $py = Find-Python310
  if ($null -eq $py) {
    Write-Warning "Python 3.10+ not found; Headroom requires Python >=3.10. Skipping install."
    exit 0
  }
  & $py[0] $py[1..($py.Length - 1)] -m pip install --user "headroom-ai[mcp]>=0.25.0"
}

if (Get-Command headroom -ErrorAction SilentlyContinue) {
  headroom mcp install --proxy-url $proxyUrl
  Write-Host "Headroom MCP ready. Restart Claude/Codex/Cursor sessions to pick it up."
} else {
  Write-Warning "Headroom executable still not on PATH after install."
}
exit 0
