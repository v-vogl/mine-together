#!/usr/bin/env bash
# Daily copy of the saved world and players, keeping the last 14 days. Run by cron (see setup.sh).
set -euo pipefail
dir=/var/lib/mine-together
day=$(date +%F)
[ -f "$dir/world.bin" ] || exit 0
cp "$dir/world.bin" "$dir/backups/world-$day.bin"
cp "$dir/players.json" "$dir/backups/players-$day.json" 2>/dev/null || true
ls -1t "$dir"/backups/world-*.bin | tail -n +15 | xargs -r rm --
ls -1t "$dir"/backups/players-*.json 2>/dev/null | tail -n +15 | xargs -r rm --
