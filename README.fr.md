<p align="center"><img src="docs/banner-palm-guard.png" alt="palm-guard" width="100%"></p>

<p align="center"><img src="https://img.shields.io/badge/platform-Linux%20·%20KDE%20Plasma%206-000000?style=flat-square" alt="platform"> <img src="https://img.shields.io/badge/wayland-yes-610089?style=flat-square" alt="wayland"> <img src="https://img.shields.io/badge/rust-2021-dea584?style=flat-square" alt="rust"> <img src="https://img.shields.io/badge/license-MIT-610089?style=flat-square" alt="license"></p>

<p align="center"><a href="README.md">🇬🇧 English version</a></p>

Anti-paume pour écran tactile sous KDE Plasma (Wayland), pensé pour dessiner au stylet dans Xournal++.

Sur mon Lenovo Yoga 7 2-in-1, quand je pose la main sur le bord gauche de l'écran pour écrire, la feuille de Xournal++ se met à défiler. libinput ne fait pas de rejet de paume sur les écrans tactiles, et je ne voulais pas couper le tactile complètement (j'aime bien déplacer la page au doigt). Du coup palm-guard se met entre l'écran et KDE et trie les touches.

## Ce qu'il fait

- **Zone morte à gauche (20 mm)** : une touche qui *commence* dans cette bande est ignorée, mais seulement quand une fenêtre Xournal++ est ouverte. Le reste du temps, tous les bords marchent normalement (fermer une fenêtre au doigt, etc.).
- **Stylet proche = tactile coupé** : dès que le stylet survole l'écran, les doigts sont ignorés, puis réactivés 0,3 s après qu'il s'éloigne.
- **Paumes signalées par l'écran** : si le pilote marque une touche comme paume (`MT_TOOL_PALM`), elle est jetée.
- Un doigt au milieu de l'écran passe comme avant : défilement, zoom, tout marche.

Côté batterie c'est neutre : le démon (en Rust, ~2,6 Mo de RAM) dort sur `poll()`, sans aucun minuteur, et ne se réveille que quand on touche l'écran ou qu'on approche le stylet.

## Comment ça marche

```
écran tactile ──grab──▶ palm-guard ──uinput──▶ « Palm Guard Touchscreen » ──▶ KDE
                            ▲
                            │ fifo /run/palm-guard.fifo
                    palm-guard-relay ◀── D-Bus ── script KWin (fenêtres Xournal++ ?)
```

- `palm-guard` (service systemd, root) attrape l'écran directement par les ioctl evdev, filtre les slots multitouch et renvoie le reste sur un clone virtuel. Pour annuler un doigt déjà parti, il le passe en `MT_TOOL_PALM` avant de le lever, comme ça libinput annule le geste au lieu de faire un clic.
- Le script KWin (`kwin/palmguard`) regarde si une fenêtre Xournal++ existe et appelle `org.palmguard.SetActive` sur le bus de session.
- `palm-guard-relay` (service utilisateur) reçoit cet appel et l'écrit dans la fifo, parce que le service root n'a pas accès au bus de session.

## Installation

Testé sur CachyOS, KDE Plasma 6.8, Lenovo Yoga 7 2-in-1 14ILL10 (Wacom HID 53FD).

```bash
git clone https://github.com/glime32/palm-guard
cd palm-guard
./install.sh
```

Il faut Rust (`sudo pacman -S rust`) : le script compile les deux binaires avec `cargo build --release` et les met dans `/usr/local/bin`. Aucune dépendance Python. Si une ancienne version Python est installée, il la remplace.

Désinstaller : `./uninstall.sh`

## Réglages

En haut de `src/filtre.rs` :

| Constante | Défaut | Rôle |
|---|---|---|
| `EDGE_LEFT_MM` | `20.0` | largeur de la zone morte à gauche, en mm |
| `XOURNAL_ONLY` | `true` | zone morte seulement si Xournal++ est ouvert |
| `PEN_BLOCKS` | `true` | couper le tactile quand le stylet est proche |
| `PEN_DELAY` | `0.3` | délai (s) avant de réactiver après le stylet |
| `PALM_MM` | `0.0` | bloquer les touches plus larges que ça (0 = off) |
| `SCREEN_MM` | `(301.5, 188.5)` | taille de l'écran, si le pilote ne donne pas la résolution |

Après modification : relancer `./install.sh` (recompile et redémarre le service).

Pour un autre logiciel que Xournal++, changer le test dans `kwin/palmguard/contents/code/main.js`.

## Tester

```bash
sudo systemctl stop palm-guard
sudo palm-guard --test
```

Le mode test affiche chaque touche avec sa position et si elle aurait été bloquée, **sans rien filtrer**. Penser à relancer le service après : `sudo systemctl start palm-guard`.

Pour comparer deux versions du filtre sur les mêmes gestes, `palm-guard --rejouer FICHIER` rejoue un enregistrement d'évènements et affiche ce qui serait envoyé à KDE (format décrit en bas de `src/main.rs`). C'est comme ça que la version Rust a été vérifiée identique à l'ancienne version Python, sur environ 180 000 évènements.

## Limites

- La distance de survol du stylet dépend du matériel, aucun logiciel ne peut l'augmenter.
- Seul le premier écran tactile trouvé est géré.
