#!/usr/bin/env python3
# palm-guard-relay : petit pont entre KWin et palm-guard.
#
# Le script KWin ne peut parler qu'en D-Bus, et palm-guard tourne en root
# sans accès au bus de session. Donc ce relais (lancé en utilisateur)
# expose org.palmguard sur le bus de session et recopie l'état dans la fifo.
import os
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

FIFO = "/run/palm-guard.fifo"
XML = ("<node><interface name=\"org.palmguard\">"
       "<method name=\"SetActive\"><arg type=\"b\" name=\"on\" direction=\"in\"/></method>"
       "</interface></node>")
INFO = Gio.DBusNodeInfo.new_for_xml(XML)


def envoyer(on):
    try:
        fd = os.open(FIFO, os.O_WRONLY | os.O_NONBLOCK)
    except OSError:
        return  # palm-guard pas lancé, pas grave
    try:
        os.write(fd, b"1\n" if on else b"0\n")
    except OSError:
        pass
    finally:
        os.close(fd)


def appel(conn, sender, path, iface, method, params, inv):
    if method == "SetActive":
        envoyer(bool(params.unpack()[0]))
    inv.return_value(None)


def bus_pret(conn, name):
    conn.register_object("/", INFO.interfaces[0], appel, None, None)


Gio.bus_own_name(Gio.BusType.SESSION, "org.palmguard",
                 Gio.BusNameOwnerFlags.NONE, bus_pret, None, None)
GLib.MainLoop().run()
