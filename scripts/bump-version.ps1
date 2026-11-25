<#
.SYNOPSIS
  Bumps the Snap It version (single source: [workspace.package] in Cargo.toml),
  moves the [Unreleased] changelog entries under the new version, and commits.

.EXAMPLE
  ./scripts/bump-version.ps1 patch        # 0.1.0 -> 0.1.1
  ./scripts/bump-version.ps1 minor        # 0.1.0 -> 0.2.0
  ./scripts/bump-version.ps1 1.0.0        # explicit version

.NOTES
  Run this on `dev`. To release, merge dev into master and tag there:
    git switch master; git merge --ff-only dev; git tag -a vX.Y.Z -m "Snap It X.Y.Z"
    git push origin master vX.Y.Z; git switch dev
#>
param([Parameter(Mandatory)][string]$Bump)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if (git status --porcelain --untracked-files=no) { throw "Commit or stash your changes first." }

$toml = Get-Content Cargo.toml -Raw
if ($toml -notmatch '(?m)^version = "(\d+)\.(\d+)\.(\d+)"') { throw "workspace version not found in Cargo.toml" }
$cur = "$($Matches[1]).$($Matches[2]).$($Matches[3])"
[int]$maj = $Matches[1]; [int]$min = $Matches[2]; [int]$pat = $Matches[3]
$new = switch ($Bump) {
    "major" { "$($maj + 1).0.0" }
    "minor" { "$maj.$($min + 1).0" }
    "patch" { "$maj.$min.$($pat + 1)" }
    default {
        if ($Bump -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { throw "Use major, minor, patch, or X.Y.Z" }
        $Bump
    }
}
if ($new -eq $cur) { throw "Version is already $cur" }

# Cargo.toml (only the first match: the workspace version)
$toml = [regex]::new('(?m)^version = "[^"]+"').Replace($toml, "version = `"$new`"", 1)
Set-Content Cargo.toml $toml -NoNewline

# CHANGELOG.md: new section under [Unreleased], and updated compare links
$date = Get-Date -Format "yyyy-MM-dd"
$cl = Get-Content CHANGELOG.md -Raw
$cl = $cl -replace '(?m)^## \[Unreleased\]\s*$', "## [Unreleased]`n`n## [$new] - $date"
$cl = $cl -replace '(?m)^\[Unreleased\]: (.+)/compare/v[^.]+\.[^.]+\.[^.]+\.\.\.dev\r?$', "[Unreleased]: `$1/compare/v$new...dev`n[$new]: `$1/compare/v$cur...v$new"
Set-Content CHANGELOG.md $cl -NoNewline

cargo update --workspace --quiet   # refresh Cargo.lock with the new version
if ($LASTEXITCODE) { throw "cargo update failed" }

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "Bump version to $new"
Write-Host "Bumped $cur -> $new. Review CHANGELOG.md, then merge to master and tag v$new." -ForegroundColor Green
