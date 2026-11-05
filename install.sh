#!/usr/bin/env bash
# Installe palm-guard. À lancer en utilisateur normal (sudo sera demandé).
set -euo pipefail
cd "$(dirname "$0")"

sudo pacman -S --needed --noconfirm python-evdev python-gobject 2>/dev/null \
  || echo "pacman indisponible : installe python-evdev et python-gobject à la main"

sudo install -Dm755 palm-guard.py /usr/local/bin/palm-guard.py
sudo install -Dm755 palm-guard-relay.py /usr/local/bin/palm-guard-relay.py
sudo install -Dm644 systemd/palm-guard.service /etc/systemd/system/palm-guard.service
sudo install -Dm644 systemd/palm-guard-relay.service /etc/systemd/user/palm-guard-relay.service
sudo systemctl daemon-reload
sudo systemctl enable --now palm-guard

d="$HOME/.local/share/kwin/scripts/palmguard"
mkdir -p "$d/contents/code"
cp kwin/palmguard/metadata.json "$d/"
cp kwin/palmguard/contents/code/main.js "$d/contents/code/"

systemctl --user daemon-reload
systemctl --user enable --now palm-guard-relay
kwriteconfig6 --file kwinrc --group Plugins --key palmguardEnabled true
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting unloadScript s palmguard >/dev/null || true
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting loadScript ss "$d/contents/code/main.js" palmguard >/dev/null
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting start

echo "palm-guard installé."
