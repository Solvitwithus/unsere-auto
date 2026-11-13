# Build the Windows station package on a Windows PC.
# Needs: Rust (https://rustup.rs, with the "Desktop development with C++" build tools), Node.js 22 and pnpm.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $MyInvocation.MyCommand.Path)\..
Push-Location web; pnpm install --frozen-lockfile; pnpm build; Pop-Location
cargo build --release -p wb-station -p wb-scale
Remove-Item dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory dist | Out-Null
Copy-Item target\release\wb-station.exe, target\release\wb-scale.exe, deploy\windows\*.ps1 dist\
Copy-Item web\out dist\web -Recurse
Compress-Archive dist\* weighbridge-station-windows.zip -Force
Write-Host "Created weighbridge-station-windows.zip"
