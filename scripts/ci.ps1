# Local CI: the same checks GitHub Actions runs. Usage: .\scripts\ci.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Step([string]$Name, [scriptblock]$Cmd) {
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Cmd
    if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: $Name" -ForegroundColor Red; exit $LASTEXITCODE }
}

Step 'cargo fmt --check' { cargo fmt --all -- --check }
Step 'cargo clippy (-D warnings)' { cargo clippy --workspace --all-targets -- -D warnings }
Step 'cargo clippy --features profile (-D warnings)' { cargo clippy -p mv_game --features profile -- -D warnings }
Step 'cargo test' { cargo test --workspace }
Write-Host 'OK: all checks passed' -ForegroundColor Green
