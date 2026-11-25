<#
.SYNOPSIS
  Builds Snap It's release downloads in dist/: a setup .exe, an .msi and a portable .zip.

.DESCRIPTION
  1. scripts/release.ps1: tests, release build, optional signing, Defender scan -> dist/snapit.exe
  2. SnapIt-<ver>-portable-x64.zip   snapit.exe + an empty `portable` file (settings stay beside it)
  3. SnapIt-<ver>-setup-x64.exe      Inno Setup 6 (installer/snapit.iss)
  4. SnapIt-<ver>-x64.msi            WiX Toolset v5 (installer/snapit.wxs)
  5. SHA256SUMS.txt                  for all of the above

  Both installers install per user (no admin prompt) with a Start menu shortcut.

  Tools: Inno Setup 6 (winget install JRSoftware.InnoSetup) and WiX v5
  (dotnet tool install --global wix --version 5.0.2 --add-source https://api.nuget.org/v3/index.json).
  CI installs both itself.

.EXAMPLE
  ./scripts/package.ps1                                  # unsigned
  ./scripts/package.ps1 -CertThumbprint 0123ABCD...      # sign the exe and both installers
  ./scripts/package.ps1 -SkipBuild                       # repackage an existing dist/snapit.exe
#>
param(
    [string]$CertThumbprint,
    [string]$PfxPath,
    [SecureString]$PfxPassword,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [string]$OutDir = "dist",
    [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# The workspace version is the single source of truth (scripts/bump-version.ps1 sets it).
$m = Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $m) { throw "version not found in Cargo.toml" }
$version = $m.Matches[0].Groups[1].Value
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "MSI needs a plain x.y.z version, got '$version'" }
Write-Host "Snap It $version" -ForegroundColor Cyan

$signing = [bool]($CertThumbprint -or $PfxPath)
if (-not $SkipBuild) {
    $relArgs = @{ OutDir = $OutDir; TimestampUrl = $TimestampUrl }
    if ($CertThumbprint) { $relArgs.CertThumbprint = $CertThumbprint }
    if ($PfxPath) { $relArgs.PfxPath = $PfxPath; $relArgs.PfxPassword = $PfxPassword }
    & "$PSScriptRoot\release.ps1" @relArgs
}
$dist = (Resolve-Path $OutDir).Path
$exe = Join-Path $dist "snapit.exe"
if (-not (Test-Path $exe)) { throw "$exe not found (run without -SkipBuild)" }

# ---- signing (same rules as release.ps1: SHA-256 + RFC 3161 timestamp, PFX never on a command line)
function Invoke-Sign([string[]]$Files) {
    if (-not $signing) { return }
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter signtool.exe |
        Where-Object FullName -match '\\x64\\' | Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)" }
    $thumb = $CertThumbprint
    $imported = $null
    if (-not $thumb) {
        $imported = Import-PfxCertificate -FilePath $PfxPath -CertStoreLocation Cert:\CurrentUser\My -Password $PfxPassword
        $thumb = $imported.Thumbprint
    }
    try {
        & $signtool.FullName sign /fd SHA256 /tr $TimestampUrl /td SHA256 /d "Snap It" /sha1 $thumb @Files
        if ($LASTEXITCODE) { throw "signing failed" }
    } finally {
        if ($imported) { Remove-Item "Cert:\CurrentUser\My\$($imported.Thumbprint)" -ErrorAction SilentlyContinue }
    }
}

$zip = Join-Path $dist "SnapIt-$version-portable-x64.zip"
$setup = Join-Path $dist "SnapIt-$version-setup-x64.exe"
$msi = Join-Path $dist "SnapIt-$version-x64.msi"

# ---- portable zip
Write-Host "==> portable zip" -ForegroundColor Cyan
$stage = Join-Path $dist "zip\SnapIt"
Remove-Item (Join-Path $dist "zip") -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
New-Item -ItemType File (Join-Path $stage "portable") | Out-Null
Copy-Item README.md, CHANGELOG.md, LICENSE $stage
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item (Join-Path $dist "zip") -Recurse -Force

# ---- setup.exe (Inno Setup)
Write-Host "==> setup.exe (Inno Setup)" -ForegroundColor Cyan
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }
& $iscc /Qp "/DAppVersion=$version" "/DDistDir=$dist" (Join-Path $root "installer\snapit.iss")
if ($LASTEXITCODE) { throw "Inno Setup failed" }

# ---- msi (WiX v5)
Write-Host "==> msi (WiX)" -ForegroundColor Cyan
$wix = (Get-Command wix -ErrorAction SilentlyContinue).Source
if (-not $wix) { throw "WiX not found (dotnet tool install --global wix --version 5.0.2)" }
# The WiX tool targets an older .NET; let it run on whatever runtime is installed.
$env:DOTNET_ROLL_FORWARD = "Major"
& $wix build (Join-Path $root "installer\snapit.wxs") -arch x64 -d "Version=$version" -d "ExePath=$exe" -d "IconPath=$(Join-Path $root 'assets\snapit.ico')" -o $msi
if ($LASTEXITCODE) { throw "WiX failed" }
Remove-Item ([IO.Path]::ChangeExtension($msi, ".wixpdb")) -ErrorAction SilentlyContinue

Invoke-Sign @($setup, $msi)

# ---- checksums
$files = @($setup, $msi, $zip, $exe)
$files | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_ -Algorithm SHA256).Hash.ToLower(), (Split-Path $_ -Leaf) } |
    Set-Content (Join-Path $dist "SHA256SUMS.txt")

Write-Host "Done:" -ForegroundColor Green
$files + (Join-Path $dist "SHA256SUMS.txt") | ForEach-Object { "  {0}  ({1:N0} KB)" -f (Split-Path $_ -Leaf), ((Get-Item $_).Length / 1KB) }
