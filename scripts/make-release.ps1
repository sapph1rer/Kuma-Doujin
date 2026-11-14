# Build KumaDoujin and publish a GitHub Release the app can update itself from.
#
#   1. set the new version in Cargo.toml   (version = "0.2.0")
#   2. powershell -ExecutionPolicy Bypass -File scripts\make-release.ps1 -Notes "what changed"
#        -NoPublish   only build dist\ (no GitHub)
#        -Draft       publish as a draft release
#
# Result: dist\KumaDoujin-v<ver>-windows-x64.zip (kumadoujin.exe, kuma-browser.exe, README.md) + .sha256.
# The app only installs a zip whose checksum file is published beside it.
param(
    [string]$Notes = "",
    [switch]$NoPublish,
    [switch]$Draft
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$Version = (Select-String -Path "Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "== building KumaDoujin $Version"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$name = "KumaDoujin-v$Version-windows-x64"
$dist = "dist/KumaDoujin"
Remove-Item -Recurse -Force "dist" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item "target/release/kumadoujin.exe", "target/release/kuma-browser.exe", "README.md" $dist

$zip = "dist/$name.zip"
Compress-Archive -Path "$dist/*" -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
Set-Content -NoNewline -Encoding ascii "$zip.sha256" "$hash  $name.zip"
Write-Host "== $zip  sha256 $hash"

if ($NoPublish) { Write-Host "built only (-NoPublish)"; exit 0 }
if (-not $Notes) { $Notes = "KumaDoujin $Version" }
$ghArgs = @("release", "create", "v$Version", $zip, "$zip.sha256", "--title", "KumaDoujin v$Version", "--notes", $Notes)
if ($Draft) { $ghArgs += "--draft" }
gh @ghArgs
if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
Write-Host "== published v$Version - running copies offer it in Settings > Updates"
