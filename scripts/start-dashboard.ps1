# Starts Qdrant (if needed), builds the Yew/WASM frontend when missing, then
# launches the kb-dashboard server. Intended for local development only.
param(
    [int]$Port = 8090
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

# Make a user-local Rust install visible to this process.
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargoCommand) {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path (Join-Path $cargoBin "cargo.exe")) {
        $env:PATH = "$cargoBin;$env:PATH"
    } else {
        throw "cargo not found. Install Rust stable first (https://rustup.rs)."
    }
}

# 1. Qdrant (idempotent).
powershell -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "start-qdrant.ps1")

# 2. Build the frontend if the bundle is missing.
$distIndex = Join-Path $RepoRoot "crates\kb-dashboard-ui\dist\index.html"
if (-not (Test-Path $distIndex)) {
    $rustupCommand = Get-Command rustup -ErrorAction SilentlyContinue
    if ($rustupCommand) {
        $targets = & rustup target list --installed
        if ($targets -notcontains "wasm32-unknown-unknown") {
            Write-Host "Adding wasm32-unknown-unknown target"
            & rustup target add wasm32-unknown-unknown
        }
    }

    $trunkPath = $null
    $trunkCommand = Get-Command trunk -ErrorAction SilentlyContinue
    if ($trunkCommand) {
        $trunkPath = $trunkCommand.Source
    } elseif (Test-Path (Join-Path $RepoRoot ".tools\trunk.exe")) {
        $trunkPath = Join-Path $RepoRoot ".tools\trunk.exe"
    }
    if (-not $trunkPath) {
        throw @"
trunk not found. Install it once, then rerun this script:
  cargo install trunk --locked
or download a release from https://github.com/trunk-rs/trunk/releases
"@
    }

    Write-Host "Building the dashboard frontend (trunk build --release)"
    Push-Location (Join-Path $RepoRoot "crates\kb-dashboard-ui")
    try {
        & $trunkPath build --release
    } finally {
        Pop-Location
    }
}

# 3. Run the dashboard.
Write-Host "Starting kb-dashboard on http://127.0.0.1:$Port"
& cargo run -p kb-dashboard -- --port $Port
