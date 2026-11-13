#!/usr/bin/env bash
# Build release packages on this machine (Linux). For the Windows station
# package use the GitHub workflow, or on a Windows PC run scripts/build-windows.ps1.
set -euo pipefail
cd "$(dirname "$0")/.."
(cd web && pnpm install --frozen-lockfile && pnpm build)
cargo build --release -p wb-server -p wb-station -p wb-scale
rm -rf dist && mkdir -p dist/server dist/station
cp target/release/wb-server dist/server/ && cp -r web/out dist/server/web && cp -r deploy/linux presets .env.example dist/server/
cp target/release/wb-station target/release/wb-scale dist/station/ && cp -r web/out dist/station/web && cp deploy/linux/wb-station.service dist/station/
tar -C dist -czf dist/weighbridge-server-linux.tar.gz server
tar -C dist -czf dist/weighbridge-station-linux.tar.gz station
echo "Packages in dist/"
