<#
.SYNOPSIS
  Publishes a new ClipVault version. Installed copies update themselves within a few hours.

.EXAMPLE
  .\release.ps1 1.1.0 "Added compact mode and fixed the image grid"

  1. Sets the version in Cargo.toml
  2. Commits all changes and tags them v1.1.0
  3. Pushes to GitHub, where the "Release" workflow builds ClipVault.exe and publishes the release
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Notes = "",
    [switch]$Wait   # wait for GitHub to finish building and show the result
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must look like 1.2.3" }
$tag = "v$Version"
if (git tag --list $tag) { throw "$tag already exists. Pick a higher version." }

$current = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
# the same number is fine as long as it was never tagged (the first release of a version)
if ([version]$Version -lt [version]$current) { throw "New version $Version must not be lower than $current." }

# 1. Version number
(Get-Content Cargo.toml -Raw) -replace '(?m)^version = "[^"]*"', "version = `"$Version`"" |
    Set-Content Cargo.toml -NoNewline

# 2. Make sure it still builds and passes before publishing anything (also updates Cargo.lock)
cargo test --release
if ($LASTEXITCODE -ne 0) { throw "Build or tests failed - nothing was published." }

# 3. Commit, tag (the tag message becomes the release notes) and push
if (-not $Notes) { $Notes = "ClipVault $Version" }
git add -A
git commit --allow-empty -m "Release $tag" -m $Notes
git tag -a $tag -m $Notes
git push
git push origin $tag

Write-Host ""
Write-Host "Pushed $tag. GitHub is building the release:" -ForegroundColor Green
Write-Host "  https://github.com/JustSad42/ClipVault/actions"
if ($Wait) {
    Start-Sleep -Seconds 5
    gh run watch (gh run list --workflow release.yml --limit 1 --json databaseId -q ".[0].databaseId") --exit-status
    gh release view $tag
}
