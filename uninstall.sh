#!/usr/bin/env bash
set -uo pipefail
systemctl --user disable --now palm-guard-relay
sudo systemctl disable --now palm-guard
sudo rm -f /usr/local/bin/palm-guard.py /usr/local/bin/palm-guard-relay.py \
  /etc/systemd/system/palm-guard.service /etc/systemd/user/palm-guard-relay.service
kwriteconfig6 --file kwinrc --group Plugins --key palmguardEnabled false
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting unloadScript s palmguard >/dev/null
rm -rf "$HOME/.local/share/kwin/scripts/palmguard"
sudo systemctl daemon-reload
echo "palm-guard désinstallé."
