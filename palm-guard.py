#!/usr/bin/env python3
# palm-guard : anti-paume pour l'écran tactile du Yoga 7 (KDE Wayland)
#
# Le script attrape l'écran tactile avant KDE, filtre les touches,
# et renvoie le reste sur un écran virtuel. Ce qu'il jette :
#   - une touche qui commence au bord gauche (seulement si Xournal++ est ouvert)
#   - tout le tactile quand le stylet est proche, + un petit délai après
#   - les touches que l'écran marque lui-même comme paume
#
# Pour tester sans filtrer (ça affiche juste ce qui serait bloqué) :
#   sudo systemctl stop palm-guard ; sudo python3 /usr/local/bin/palm-guard.py --test
# et ne pas oublier `sudo systemctl start palm-guard` après.

import os
import sys
import time
import select
import evdev
from evdev import ecodes as e, AbsInfo

# --- réglages ---
BORD_MM = {"gauche": 20, "droite": 0, "haut": 0, "bas": 0}  # largeur de la zone morte
XOURNAL_SEUL = True        # la zone morte ne sert que si Xournal++ est ouvert
STYLET_BLOQUE = True       # stylet au-dessus de l'écran = tactile coupé
STYLET_DELAI = 0.3         # en secondes, après que le stylet s'éloigne
PAUME_MM = 0               # bloque les touches plus larges que ça (0 = désactivé)
ECRAN_MM = (301.5, 188.5)  # 14 pouces en 16:10, sert si le pilote ne donne pas la résolution
NOM = "Palm Guard Touchscreen"
FIFO = "/run/palm-guard.fifo"  # le relais KWin écrit 1/0 dedans

TOOLS = [e.BTN_TOOL_FINGER, e.BTN_TOOL_DOUBLETAP, e.BTN_TOOL_TRIPLETAP,
         e.BTN_TOOL_QUADTAP, e.BTN_TOOL_QUINTTAP]
FINGER = 0
PALM = 2
# tous les codes ABS_MT_* qu'on recopie tels quels
MT_MIN = e.ABS_MT_TOUCH_MAJOR
MT_MAX = e.ABS_MT_TOOL_Y


class Slot:
    """Un doigt (slot multitouch)."""

    def __init__(self):
        self.actif = False   # le doigt est posé sur l'écran réel
        self.etat = None     # None, "ok" (renvoyé) ou "bloque"
        self.vals = {}       # dernières valeurs connues
        self.chg = {}        # valeurs changées depuis la dernière frame
        self.debut = False
        self.fin = False


class Geo:
    """Conversion unités de l'écran -> millimètres."""

    def __init__(self, dev):
        ax = dev.absinfo(e.ABS_MT_POSITION_X)
        ay = dev.absinfo(e.ABS_MT_POSITION_Y)
        self.x0, self.y0 = ax.min, ay.min
        self.ux = ax.resolution or (ax.max - ax.min) / ECRAN_MM[0]
        self.uy = ay.resolution or (ay.max - ay.min) / ECRAN_MM[1]
        self.w = (ax.max - ax.min) / self.ux
        self.h = (ay.max - ay.min) / self.uy

    def mm(self, x, y):
        return (x - self.x0) / self.ux, (y - self.y0) / self.uy

    def au_bord(self, x, y):
        mx, my = self.mm(x, y)
        return (mx < BORD_MM["gauche"] or mx > self.w - BORD_MM["droite"]
                or my < BORD_MM["haut"] or my > self.h - BORD_MM["bas"])


class Filtre:
    def __init__(self, out, geo, keys, abss, test=False):
        self.out = out
        self.geo = geo
        self.keys = keys
        self.abss = abss
        self.test = test
        self.slots = {}
        self.cur = 0
        self.vid = 0          # tracking id qu'on donne sur l'écran virtuel
        self.perdu = False    # le noyau a sauté des évènements (SYN_DROPPED)
        self.proche = False
        self.parti = -100.0
        self.xournal = False

    def slot(self, n):
        if n not in self.slots:
            self.slots[n] = Slot()
        return self.slots[n]

    def emit(self, t, c, v):
        if self.out:
            self.out.write(t, c, v)

    def syn(self):
        if self.out:
            self.out.syn()

    # --- écran tactile ---
    def ecran(self, ev):
        if ev.type == e.EV_SYN:
            if ev.code == e.SYN_DROPPED:
                self.perdu = True
            elif ev.code == e.SYN_REPORT:
                if self.perdu:
                    # on ne sait plus où on en est : on lâche tout et on attend
                    # que les doigts se relèvent
                    self.perdu = False
                    self.tout_annuler()
                    for s in self.slots.values():
                        s.etat = "bloque"
                        s.debut = s.fin = False
                        s.chg = {}
                else:
                    self.frame()
            return
        if self.perdu or ev.type != e.EV_ABS:
            return
        if ev.code == e.ABS_MT_SLOT:
            self.cur = ev.value
            return
        s = self.slot(self.cur)
        if ev.code == e.ABS_MT_TRACKING_ID:
            if ev.value < 0:
                if s.debut:
                    s.debut = False   # posé et levé dans la même frame
                else:
                    s.fin = True
                s.actif = False
            else:
                if s.actif:
                    s.fin = True
                s.actif = True
                s.debut = True
            return
        if MT_MIN <= ev.code <= MT_MAX:
            s.vals[ev.code] = ev.value
            s.chg[ev.code] = ev.value

    def raison(self, s, now, en_cours=False):
        """Pourquoi bloquer ce doigt (None = on le laisse passer)."""
        if s.vals.get(e.ABS_MT_TOOL_TYPE, FINGER) == PALM:
            return "paume"
        if PAUME_MM and s.vals.get(e.ABS_MT_TOUCH_MAJOR, 0) / self.geo.ux > PAUME_MM:
            return "trop large"
        if en_cours:
            # un doigt déjà accepté qui glisse vers le bord, on le garde
            return None
        if STYLET_BLOQUE and (self.proche or now - self.parti < STYLET_DELAI):
            return "stylet proche"
        x = s.vals.get(e.ABS_MT_POSITION_X)
        y = s.vals.get(e.ABS_MT_POSITION_Y)
        bords = self.xournal or not XOURNAL_SEUL
        if bords and x is not None and y is not None and self.geo.au_bord(x, y):
            return "bord"
        return None

    def log(self, s, why):
        if not self.test:
            return
        mx, my = self.geo.mm(s.vals.get(e.ABS_MT_POSITION_X, 0),
                             s.vals.get(e.ABS_MT_POSITION_Y, 0))
        maj = s.vals.get(e.ABS_MT_TOUCH_MAJOR)
        taille = "?" if maj is None else f"{maj / self.geo.ux:.0f}mm"
        etat = f"BLOQUEE ({why})" if why else "OK"
        print(f"Touche x={mx:.0f}mm y={my:.0f}mm taille={taille} -> {etat}", flush=True)

    def frame(self):
        now = time.monotonic()
        lever = []
        for n in sorted(self.slots):
            s = self.slots[n]
            if s.fin:
                if s.etat == "ok":
                    self.emit(e.EV_ABS, e.ABS_MT_SLOT, n)
                    self.emit(e.EV_ABS, e.ABS_MT_TRACKING_ID, -1)
                s.etat = None
            if s.debut:
                why = self.raison(s, now)
                self.log(s, why)
                if why:
                    s.etat = "bloque"
                else:
                    s.etat = "ok"
                    self.vid = (self.vid + 1) % 60000
                    self.emit(e.EV_ABS, e.ABS_MT_SLOT, n)
                    self.emit(e.EV_ABS, e.ABS_MT_TOOL_TYPE, FINGER)
                    self.emit(e.EV_ABS, e.ABS_MT_TRACKING_ID, self.vid)
                    for c, v in s.vals.items():
                        if c in self.abss and c != e.ABS_MT_TOOL_TYPE:
                            self.emit(e.EV_ABS, c, v)
            elif s.etat == "ok" and s.chg:
                self.emit(e.EV_ABS, e.ABS_MT_SLOT, n)
                if self.raison(s, now, en_cours=True):
                    # l'écran a changé d'avis (paume) : on annule proprement
                    self.emit(e.EV_ABS, e.ABS_MT_TOOL_TYPE, PALM)
                    s.etat = "bloque"
                    lever.append(n)
                else:
                    for c, v in s.chg.items():
                        if c in self.abss and c != e.ABS_MT_TOOL_TYPE:
                            self.emit(e.EV_ABS, c, v)
            s.debut = s.fin = False
            s.chg = {}
        self.pointeur()
        self.syn()
        if lever:
            self.lever(lever)

    def pointeur(self):
        # BTN_TOUCH, BTN_TOOL_* et ABS_X/Y doivent suivre les doigts acceptés,
        # sinon libinput se perd
        ok = [n for n in sorted(self.slots) if self.slots[n].etat == "ok"]
        nb = len(ok)
        if e.BTN_TOUCH in self.keys:
            self.emit(e.EV_KEY, e.BTN_TOUCH, 1 if nb else 0)
        for i, k in enumerate(TOOLS):
            if k in self.keys:
                on = nb == i + 1 or (i == 4 and nb > 5)
                self.emit(e.EV_KEY, k, 1 if on else 0)
        if ok:
            s = self.slots[ok[0]]
            for mt, st in ((e.ABS_MT_POSITION_X, e.ABS_X),
                           (e.ABS_MT_POSITION_Y, e.ABS_Y),
                           (e.ABS_MT_PRESSURE, e.ABS_PRESSURE)):
                if st in self.abss and mt in s.vals:
                    self.emit(e.EV_ABS, st, s.vals[mt])

    def tout_annuler(self):
        # marquer en paume avant de lever = libinput annule le geste
        # au lieu de faire un clic
        ok = [n for n in sorted(self.slots) if self.slots[n].etat == "ok"]
        if not ok:
            return
        for n in ok:
            self.emit(e.EV_ABS, e.ABS_MT_SLOT, n)
            self.emit(e.EV_ABS, e.ABS_MT_TOOL_TYPE, PALM)
            self.slots[n].etat = "bloque"
        self.syn()
        self.lever(ok)

    def lever(self, ns):
        for n in ns:
            self.emit(e.EV_ABS, e.ABS_MT_SLOT, n)
            self.emit(e.EV_ABS, e.ABS_MT_TRACKING_ID, -1)
        self.pointeur()
        self.syn()

    # --- stylet ---
    def stylet(self, ev):
        if ev.type != e.EV_KEY or ev.code not in (e.BTN_TOOL_PEN, e.BTN_TOOL_RUBBER):
            return
        if ev.value:
            if not self.proche:
                self.proche = True
                if self.test:
                    print("Stylet proche", flush=True)
                if STYLET_BLOQUE:
                    self.tout_annuler()
        elif self.proche:
            self.proche = False
            self.parti = time.monotonic()
            if self.test:
                print("Stylet loin", flush=True)


def ouvrir_fifo():
    try:
        os.unlink(FIFO)
    except FileNotFoundError:
        pass
    os.mkfifo(FIFO)
    os.chmod(FIFO, 0o666)  # le relais tourne en utilisateur, pas en root
    # O_RDWR pour que la fifo ne renvoie pas EOF quand le relais redémarre
    return os.open(FIFO, os.O_RDWR | os.O_NONBLOCK)


def lire_fifo(fd, f):
    try:
        data = os.read(fd, 4096).decode("ascii", "ignore")
    except BlockingIOError:
        return
    etats = [c for c in data if c in "01"]
    if etats:
        f.xournal = etats[-1] == "1"
        if f.test:
            print("Xournal++ ouvert" if f.xournal else "Xournal++ fermé", flush=True)


def trouver():
    """Cherche l'écran tactile et le stylet (les deux en INPUT_PROP_DIRECT)."""
    ecran = stylet = None
    for p in evdev.list_devices():
        try:
            d = evdev.InputDevice(p)
        except OSError:
            continue
        caps = d.capabilities(absinfo=False)
        keys = caps.get(e.EV_KEY, [])
        abss = caps.get(e.EV_ABS, [])
        direct = e.INPUT_PROP_DIRECT in d.input_props()
        if direct and d.name != NOM:  # ne pas s'attraper soi-même
            if e.BTN_TOOL_PEN in keys:
                if stylet is None:
                    stylet = d
                    continue
            elif e.ABS_MT_POSITION_X in abss and e.ABS_MT_SLOT in abss:
                if ecran is None:
                    ecran = d
                    continue
        d.close()
    return ecran, stylet


def virtuel(dev):
    """Crée un clone de l'écran avec uinput (même ids, mêmes axes)."""
    caps = dev.capabilities()
    caps.pop(e.EV_SYN, None)
    caps.pop(e.EV_FF, None)
    # ABS_MT_TOOL_TYPE est ajouté même si l'écran ne l'a pas : c'est lui
    # qui permet d'annuler un doigt
    absl = [(c, i) for c, i in caps.get(e.EV_ABS, []) if c != e.ABS_MT_TOOL_TYPE]
    absl.append((e.ABS_MT_TOOL_TYPE, AbsInfo(0, 0, 15, 0, 0, 0)))
    caps[e.EV_ABS] = absl
    i = dev.info
    out = evdev.UInput(caps, name=NOM, vendor=i.vendor, product=i.product,
                       version=i.version, bustype=i.bustype,
                       input_props=dev.input_props())
    return out, {c for c, _ in absl}, set(caps.get(e.EV_KEY, []))


def main():
    test = "--test" in sys.argv
    ecran, stylet = trouver()
    if ecran is None:
        print("Ecran tactile introuvable", flush=True)
        sys.exit(1)
    geo = Geo(ecran)
    nom_stylet = stylet.name if stylet else "aucun"
    print(f"Ecran : {ecran.name} ({geo.w:.0f} x {geo.h:.0f} mm)", flush=True)
    print(f"Stylet : {nom_stylet}", flush=True)
    if test:
        out, abss, keys = None, set(), set()
        print("Mode test : touche l'écran, Ctrl+C pour quitter", flush=True)
    else:
        out, abss, keys = virtuel(ecran)
        ecran.grab()
    f = Filtre(out, geo, keys, abss, test)
    devs = {ecran.fd: ecran}
    if stylet:
        devs[stylet.fd] = stylet
    fifo = ouvrir_fifo()
    try:
        # tout est bloquant sur select : le script dort tant qu'on ne touche rien
        while True:
            r, _, _ = select.select(list(devs) + [fifo], [], [])
            for fd in r:
                if fd == fifo:
                    lire_fifo(fifo, f)
                    continue
                d = devs[fd]
                try:
                    evs = list(d.read())
                except BlockingIOError:
                    continue
                for ev in evs:
                    if d is ecran:
                        f.ecran(ev)
                    else:
                        f.stylet(ev)
    except KeyboardInterrupt:
        pass
    finally:
        if out:
            try:
                ecran.ungrab()
            except OSError:
                pass
            out.close()


if __name__ == "__main__":
    main()
