#!/usr/bin/env bash
# One-time setup of a fresh Ubuntu 24.04 server for Mine Together. deploy.ps1 uploads this folder;
# then run as root:   bash /tmp/mt/deploy/setup.sh your.domain.example
set -euo pipefail
DOMAIN="${1:?usage: setup.sh DOMAIN}"
here="$(cd "$(dirname "$0")" && pwd)"

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get -y upgrade
DEBIAN_FRONTEND=noninteractive apt-get -y install build-essential curl rsync caddy ufw unattended-upgrades

# Firewall: only SSH and the website.
ufw allow OpenSSH
ufw allow 80/tcp
ufw allow 443/tcp
ufw --force enable

# A system user that runs (and builds) the game server, without a login shell.
id minetogether >/dev/null 2>&1 ||
	useradd --system --create-home --home-dir /opt/mine-together --shell /usr/sbin/nologin minetogether
mkdir -p /opt/mine-together/build /var/lib/mine-together/backups /var/www/mine-together
chown -R minetogether: /opt/mine-together /var/lib/mine-together

# Rust, for building the server on this machine.
[ -x /opt/mine-together/.cargo/bin/cargo ] ||
	sudo -u minetogether -H bash -c 'curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'

install -m 644 "$here/mine-together.service" /etc/systemd/system/mine-together.service
sed "s/{DOMAIN}/$DOMAIN/" "$here/Caddyfile" >/etc/caddy/Caddyfile
install -m 755 "$here/backup.sh" /usr/local/bin/mine-together-backup
echo '30 4 * * * minetogether /usr/local/bin/mine-together-backup' >/etc/cron.d/mine-together-backup
systemctl daemon-reload
systemctl enable mine-together
systemctl restart caddy

# Log in with keys only (this script is run over a key login, so that works already).
echo 'PasswordAuthentication no' >/etc/ssh/sshd_config.d/00-keys-only.conf
systemctl reload ssh

echo "Setup done. Now deploy the game with deploy.ps1."
