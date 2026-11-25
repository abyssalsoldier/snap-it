<#
.SYNOPSIS
  Builds a release snapit.exe, optionally Authenticode-signs it, and scans it with Microsoft Defender.

.EXAMPLE
  ./scripts/release.ps1                                   # build + hash + Defender scan (unsigned)
  ./scripts/release.ps1 -CertThumbprint 0123ABCD...       # sign with a cert from the user/machine store
  ./scripts/release.ps1 -PfxPath code.pfx -PfxPassword (Read-Host -AsSecureString)

.NOTES
  EDR/AV hygiene this script keeps:
  - no packers (UPX), no obfuscation, no self-extracting archive;
  - icon + version resource + manifest are embedded by build.rs;
  - signing uses SHA-256 with an RFC 3161 timestamp so the signature outlives the cert;
  - the output is scanned with Defender before you ship it.
#>
param(
    [string]$CertThumbprint,
    [string]$PfxPath,
    [SecureString]$PfxPassword,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [string]$OutDir = "dist"
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

Write-Host "==> cargo test" -ForegroundColor Cyan
cargo test --workspace --quiet
if ($LASTEXITCODE) { throw "tests failed" }

Write-Host "==> cargo build --release" -ForegroundColor Cyan
cargo build --release -p snap-it
if ($LASTEXITCODE) { throw "build failed" }

New-Item -ItemType Directory -Force $OutDir | Out-Null
$exe = Join-Path $OutDir "snapit.exe"
Copy-Item target\release\snapit.exe $exe -Force

if ($CertThumbprint -or $PfxPath) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter signtool.exe |
        Where-Object FullName -match '\\x64\\' | Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)" }
    # A PFX is imported into the user certificate store for the duration of signing,
    # so its password never appears on a process command line (EDRs log those).
    $imported = $null
    if (-not $CertThumbprint) {
        $imported = Import-PfxCertificate -FilePath $PfxPath -CertStoreLocation Cert:\CurrentUser\My -Password $PfxPassword
        $CertThumbprint = $imported.Thumbprint
    }
    $signArgs = @("sign", "/fd", "SHA256", "/tr", $TimestampUrl, "/td", "SHA256", "/d", "Snap It", "/sha1", $CertThumbprint)
    Write-Host "==> signing" -ForegroundColor Cyan
    try {
        & $signtool.FullName @signArgs $exe
        if ($LASTEXITCODE) { throw "signing failed" }
    } finally {
        if ($imported) { Remove-Item "Cert:\CurrentUser\My\$($imported.Thumbprint)" -ErrorAction SilentlyContinue }
    }
    & $signtool.FullName verify /pa /v $exe | Select-String "Successfully verified|Issued to"
} else {
    Write-Warning "Unsigned build. Sign release binaries (-CertThumbprint or -PfxPath) to avoid SmartScreen and EDR reputation prompts."
}

$hash = (Get-FileHash $exe -Algorithm SHA256).Hash
"$hash  snapit.exe" | Set-Content (Join-Path $OutDir "snapit.exe.sha256")
Write-Host "SHA-256: $hash"

$mp = @("$env:ProgramFiles\Windows Defender\MpCmdRun.exe") +
      (Get-ChildItem "$env:ProgramData\Microsoft\Windows Defender\Platform" -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName "MpCmdRun.exe" })
$mp = $mp | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($mp) {
    Write-Host "==> Defender scan" -ForegroundColor Cyan
    & $mp -Scan -ScanType 3 -File (Resolve-Path $exe) -DisableRemediation
    if ($LASTEXITCODE -ne 0) { throw "Defender flagged $exe (exit $LASTEXITCODE)" }
} else {
    Write-Warning "MpCmdRun.exe not found; skipped the Defender scan."
}
Write-Host "Done: $exe" -ForegroundColor Green
