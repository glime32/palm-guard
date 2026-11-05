// Palm Guard : prévient palm-guard quand une fenêtre Xournal++ est ouverte.
// Pas de boucle : on ne recalcule que quand une fenêtre apparaît,
// disparaît ou prend le focus.
function estXournal(w) {
    var c = String(w.resourceClass || "") + " " + String(w.desktopFileName || "");
    return c.toLowerCase().indexOf("xournal") !== -1;
}

// `partie` = la fenêtre en train de se fermer (elle est encore dans la liste)
function verifier(partie) {
    var id = partie ? String(partie.internalId) : "";
    var liste = workspace.windowList();
    var on = false;
    for (var i = 0; i < liste.length; i++) {
        if (String(liste[i].internalId) !== id && estXournal(liste[i])) {
            on = true;
            break;
        }
    }
    callDBus("org.palmguard", "/", "org.palmguard", "SetActive", on);
}

workspace.windowAdded.connect(function () { verifier(null); });
workspace.windowRemoved.connect(function (w) { verifier(w); });
workspace.windowActivated.connect(function () { verifier(null); });
verifier(null);
