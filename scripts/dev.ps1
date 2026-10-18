# Edit-and-see loop for the Open NTRIP Client UI.
#
#   .\scripts\dev.ps1            build (debug) and run the app
#   .\scripts\dev.ps1 -Check     run the four gates CI runs (fmt, clippy, tests, release build)
#
# Works from a local clone or straight from a network share: builds never
# write into the repo folder, they go to a local target dir (CARGO_TARGET_DIR),
# which is also what keeps share builds from being slow or flaky.
param([switch]$Check)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'cargo-target\open-ntrip-client'
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo not found - install Rust from https://rustup.rs (the pinned toolchain installs itself on first build)'
}

Push-Location $repo
try {
    if ($Check) {
        cargo fmt --all --check;                              if ($LASTEXITCODE) { throw 'fmt: run "cargo fmt --all" to fix' }
        cargo clippy --workspace --all-targets -- -D warnings; if ($LASTEXITCODE) { throw 'clippy failed' }
        cargo test --workspace;                               if ($LASTEXITCODE) { throw 'tests failed' }
        cargo build --workspace --release;                    if ($LASTEXITCODE) { throw 'release build failed' }
        Write-Host 'All gates passed.' -ForegroundColor Green
    } else {
        cargo run --bin OpenNtripClient
    }
} finally {
    Pop-Location
}
