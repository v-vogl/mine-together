#!/usr/bin/env bash
# Runs on the server (started by deploy.ps1): build the uploaded server source, install it and the
# client, restart the game server. The build folder is kept, so later builds only redo what changed.
set -euo pipefail
up=/tmp/mt
build=/opt/mine-together/build

rsync -a --delete "$up/server/src/" "$build/src/"
cp "$up/server/Cargo.toml" "$up/server/Cargo.lock" "$build/"
chown -R minetogether: "$build"
sudo -u minetogether -H bash -c "cd $build && ~/.cargo/bin/cargo build --release"

install -o minetogether -m 755 "$build/target/release/mine-together-server" /opt/mine-together/mine-together-server
rsync -a --delete "$up/client/dist/" /var/www/mine-together/
systemctl restart mine-together
sleep 2
systemctl --no-pager --lines=5 status mine-together
