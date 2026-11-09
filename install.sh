#!/usr/bin/env bash
# Installe palm-guard. À lancer en utilisateur normal (sudo sera demandé).
set -euo pipefail
cd "$(dirname "$0")"

command -v cargo >/dev/null || { echo "cargo introuvable : installe rust (sudo pacman -S rust)"; exit 1; }
cargo build --release --locked

sudo install -Dm755 target/release/palm-guard /usr/local/bin/palm-guard
sudo install -Dm755 target/release/palm-guard-relay /usr/local/bin/palm-guard-relay
sudo install -Dm644 systemd/palm-guard.service /etc/systemd/system/palm-guard.service
sudo install -Dm644 systemd/palm-guard-relay.service /etc/systemd/user/palm-guard-relay.service
# ancienne version Python
sudo rm -f /usr/local/bin/palm-guard.py /usr/local/bin/palm-guard-relay.py
sudo systemctl daemon-reload
sudo systemctl enable palm-guard
sudo systemctl restart palm-guard

d="$HOME/.local/share/kwin/scripts/palmguard"
mkdir -p "$d/contents/code"
cp kwin/palmguard/metadata.json "$d/"
cp kwin/palmguard/contents/code/main.js "$d/contents/code/"

systemctl --user daemon-reload
systemctl --user enable palm-guard-relay
systemctl --user restart palm-guard-relay
kwriteconfig6 --file kwinrc --group Plugins --key palmguardEnabled true
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting unloadScript s palmguard >/dev/null || true
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting loadScript ss "$d/contents/code/main.js" palmguard >/dev/null
busctl --user call org.kde.KWin /Scripting org.kde.kwin.Scripting start

echo "palm-guard installé."
