-- Fassung 2: zwei Flaggen als eigene Spalten.
--
-- `flaggen` bleibt als Textliste für alles Übrige (eigene Schlüsselwörter des
-- Servers, $Forwarded, Anbieter-Eigenheiten). Gelesen und Markiert bekommen
-- eigene Spalten, weil die Gesprächsliste nach ihnen gruppiert und filtert:
-- `instr(flaggen, 'gelesen')` lässt sich nicht indizieren und trifft ausserdem
-- jedes Schlüsselwort, in dem die Zeichenfolge vorkommt.
--
-- Eigene Datei statt Änderung an 001: Auf einem Gerät, das 001 schon
-- angewandt hat, läuft eine geänderte 001 nicht erneut — dort entstünde ein
-- Schema, das es nirgends sonst gibt.

ALTER TABLE nachrichten ADD COLUMN gelesen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE nachrichten ADD COLUMN markiert INTEGER NOT NULL DEFAULT 0;

-- Der Index, den die Ungelesen-Zählung braucht.
CREATE INDEX nachrichten_ungelesen ON nachrichten (ordner_id, gelesen);

-- Der normalisierte Betreff — ohne „Re:", „AW:", „WG:", kleingeschrieben.
-- Gespeichert und nicht bei jeder Abfrage gerechnet: Der Betreff-Rückfall des
-- Threadings sucht darüber, und SQLite kann eine Funktion im WHERE nicht
-- indizieren.
ALTER TABLE nachrichten ADD COLUMN betreff_normalisiert TEXT NOT NULL DEFAULT '';

CREATE INDEX nachrichten_betreff_norm ON nachrichten (betreff_normalisiert, gesendet_am DESC);
