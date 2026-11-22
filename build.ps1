#Requires -Version 5.1
<#
    Build DeskBeam into a single portable executable.

    Usage:
      .\build.ps1              release build + copy to .\DeskBeam.exe
      .\build.ps1 -Test        run unit tests and the loopback self-test first
      .\build.ps1 -Debug       debug build
#>
param(
    [switch]$Test,
    [switch]$Debug
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# A freshly installed rustup may not be on this process's PATH yet.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path (Join-Path $env:USERPROFILE '.cargo') 'bin'
    if (Test-Path (Join-Path $cargoBin 'cargo.exe')) {
        $env:Path = "$cargoBin;$env:Path"
    }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo was not found. Install Rust (rustup) and reopen the terminal.'
}

$cargoProfile = if ($Debug) { 'dev' } else { 'release' }
$outDir = if ($Debug) { 'debug' } else { 'release' }
$cargoArgs = @('build', '--profile', $cargoProfile)

if ($Test) {
    Write-Host '== cargo test ==' -ForegroundColor Cyan
    cargo test --profile $cargoProfile
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
}

Write-Host "== cargo $($cargoArgs -join ' ') ==" -ForegroundColor Cyan
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$exe = Join-Path $PSScriptRoot "target\$outDir\deskbeam.exe"
if (-not (Test-Path $exe)) { throw "expected binary not found: $exe" }

$out = Join-Path $PSScriptRoot 'DeskBeam.exe'
Copy-Item $exe $out -Force
$size = [math]::Round((Get-Item $out).Length / 1MB, 1)
Write-Host "Portable build: $out ($size MB)" -ForegroundColor Green

if ($Test) {
    Write-Host '== loopback self-test ==' -ForegroundColor Cyan
    # The release binary uses the Windows GUI subsystem, so `&` returns
    # immediately. Use Start-Process -Wait to obtain the real exit code.
    $proc = Start-Process -FilePath $out -ArgumentList '--selftest' -PassThru -Wait
    if (Test-Path 'deskbeam-selftest.txt') { Get-Content 'deskbeam-selftest.txt' }
    if ($proc.ExitCode -ne 0) { throw "self-test failed (exit $($proc.ExitCode))" }
    Write-Host 'Self-test passed.' -ForegroundColor Green
}
