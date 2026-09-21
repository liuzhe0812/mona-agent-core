# Windows PowerShell 5.1+ / PowerShell 7. No real model or paid API is invoked.
param([switch] $NativeTauri)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
New-Item -ItemType Directory -Force -Path "verification" | Out-Null
Start-Transcript -Path "verification/verify.log" -Force | Out-Null
$Passed = $false
function Run-Native {
    param([string] $Program, [string[]] $Arguments)
    Write-Host ("> " + $Program + " " + ($Arguments -join " "))
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program exited with code $LASTEXITCODE" }
}
try {
    Write-Host ("Verification started: " + (Get-Date).ToUniversalTime().ToString("o"))
    Get-Command node -ErrorAction Stop | Out-Null
    Run-Native "node" @("scripts/test-clients.mjs")
    Get-Command cargo -ErrorAction Stop | Out-Null
    Get-Command rustc -ErrorAction Stop | Out-Null
    Run-Native "rustc" @("-Vv")
    Run-Native "cargo" @("-V")
    & rustc -Vv | Out-File "verification/rustc-version.txt" -Encoding utf8
    & cargo -V | Out-File "verification/cargo-version.txt" -Encoding utf8
    if (-not (Test-Path Cargo.lock)) { Run-Native "cargo" @("generate-lockfile") }
    Run-Native "cargo" @("fmt", "--all")
    Run-Native "cargo" @("fmt", "--all", "--", "--check")
    Run-Native "cargo" @("check", "--workspace", "--all-targets", "--locked")
    Run-Native "cargo" @("test", "--workspace", "--all-targets", "--locked")
    Run-Native "cargo" @("test", "--workspace", "--doc", "--locked")
    Run-Native "cargo" @("clippy", "--workspace", "--all-targets", "--locked")
    Run-Native "cargo" @("run", "--locked", "-p", "demo", "--bin", "minimal")
    Run-Native "cargo" @("run", "--locked", "-p", "demo", "--bin", "memory")
    Run-Native "cargo" @("run", "--locked", "-p", "demo", "--bin", "planner")
    Run-Native "cargo" @("run", "--locked", "-p", "demo", "--bin", "generic_extensions")
    if ($NativeTauri) {
        Run-Native "cargo" @("check", "-p", "tauri-plugin-bridge", "--features", "tauri", "--all-targets", "--locked")
        Run-Native "cargo" @("check", "-p", "tauri-composition", "--features", "tauri", "--locked")
    }
    $Passed = $true
    Write-Host "Offline verification finished. Keep Cargo.lock; review warnings; validate the real provider separately."
}
catch {
    Write-Host ("VERIFICATION FAILED: " + $_.Exception.Message)
}
finally {
    $Status = "failed"
    if ($Passed) { $Status = "passed" }
    @{status=$Status; scope="rust-workspace-default-features,js-clients,offline-demos"; native_tauri_requested=[bool]$NativeTauri; live_model_tested=$false} |
        ConvertTo-Json | Out-File "verification/status.json" -Encoding utf8
    Stop-Transcript | Out-Null
}
if (-not $Passed) { exit 1 }
