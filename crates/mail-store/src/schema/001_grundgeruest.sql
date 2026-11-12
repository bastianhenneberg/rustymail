-- Fassung 1: das Gerüst. Jede weitere Änderung kommt als eigene Datei dazu,
-- diese hier wird nie wieder angefasst — eine geänderte Wanderung läuft auf
-- Geräten, die sie schon hinter sich haben, nicht erneut.

CREATE TABLE konten (
    id              TEXT PRIMARY KEY,
    beschriftung    TEXT NOT NULL,
    adresse         TEXT NOT NULL,
    protokoll       TEXT NOT NULL CHECK (protokoll IN ('imap', 'jmap')),
    zugang_json     TEXT,
    -- KEIN Passwort, KEIN Token. Die liegen im Tresor der Plattform; hier
    -- steht nur, unter welcher Kennung sie dort zu finden sind.
    tresor_kennung  TEXT,
    zuletzt_geprueft_am INTEGER,
    gesundheit      TEXT
) STRICT;

CREATE TABLE ordner (
    id              TEXT PRIMARY KEY,
    konto_id        TEXT NOT NULL REFERENCES konten(id) ON DELETE CASCADE,
    pfad            TEXT NOT NULL,
    trenner         TEXT NOT NULL DEFAULT '/',
    rolle           TEXT,
    gesamt          INTEGER NOT NULL DEFAULT 0,
    ungelesen       INTEGER NOT NULL DEFAULT 0,
    -- Der Sync-Zustand. Ohne diese drei Spalten ist jeder Abgleich ein
    -- vollständiger Durchlauf, und ein Serverwechsel bleibt unbemerkt.
    uidvalidity     INTEGER,
    highestmodseq   INTEGER,
    abgeglichen_am  INTEGER,
    UNIQUE (konto_id, pfad)
) STRICT;

CREATE TABLE nachrichten (
    id              TEXT PRIMARY KEY,
    konto_id        TEXT NOT NULL REFERENCES konten(id) ON DELETE CASCADE,
    ordner_id       TEXT NOT NULL REFERENCES ordner(id) ON DELETE CASCADE,
    -- Bei IMAP die UID, bei JMAP die Email-Id. Deshalb TEXT.
    server_kennung  TEXT NOT NULL,
    message_id      TEXT,
    thread_schluessel TEXT NOT NULL,
    betreff         TEXT NOT NULL DEFAULT '',
    von_name        TEXT,
    von_adresse     TEXT NOT NULL DEFAULT '',
    gesendet_am     INTEGER NOT NULL,
    empfangen_am    INTEGER,
    groesse         INTEGER NOT NULL DEFAULT 0,
    flaggen         TEXT NOT NULL DEFAULT '',
    hat_anhang      INTEGER NOT NULL DEFAULT 0,
    -- Erst die Kopfzeilen, der Körper kommt beim Öffnen nach. Eine Liste
    -- soll nicht auf 200 Textkörper warten müssen.
    koerper_geholt  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (ordner_id, server_kennung)
) STRICT;

CREATE INDEX nachrichten_thread ON nachrichten (thread_schluessel, gesendet_am DESC);
CREATE INDEX nachrichten_ordner_zeit ON nachrichten (ordner_id, gesendet_am DESC);
CREATE INDEX nachrichten_message_id ON nachrichten (message_id);

CREATE TABLE koerper (
    nachricht_id    TEXT PRIMARY KEY REFERENCES nachrichten(id) ON DELETE CASCADE,
    text            TEXT,
    html            TEXT,
    roh             BLOB
) STRICT;

CREATE TABLE anhaenge (
    id              TEXT PRIMARY KEY,
    nachricht_id    TEXT NOT NULL REFERENCES nachrichten(id) ON DELETE CASCADE,
    dateiname       TEXT NOT NULL,
    medientyp       TEXT NOT NULL,
    groesse         INTEGER NOT NULL DEFAULT 0,
    -- Inline-Bilder: die Content-Id, auf die der Körper per cid: verweist.
    content_id      TEXT,
    inline          INTEGER NOT NULL DEFAULT 0,
    pfad            TEXT
) STRICT;

-- Die Suche. FTS5 als eigene Tabelle und nicht als Index auf `nachrichten`:
-- ein externes Inhaltsverzeichnis wäre kleiner, muss aber bei jedem Löschen
-- von Hand nachgeführt werden, und ein verpasstes Löschen liefert Treffer auf
-- Nachrichten, die es nicht mehr gibt.
CREATE VIRTUAL TABLE suche USING fts5 (
    nachricht_id UNINDEXED,
    betreff,
    von,
    an,
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Was lokal passiert ist, aber noch nicht beim Server angekommen. Jede
-- Operation muss idempotent wiederholbar sein: Das Netz bricht mitten im
-- Senden weg, und beim nächsten Start weiß niemand, ob der Server sie
-- gesehen hat.
CREATE TABLE ausstehend (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    konto_id        TEXT NOT NULL REFERENCES konten(id) ON DELETE CASCADE,
    art             TEXT NOT NULL,
    nutzlast_json   TEXT NOT NULL,
    angelegt_am     INTEGER NOT NULL,
    versuche        INTEGER NOT NULL DEFAULT 0,
    letzter_fehler  TEXT
) STRICT;

CREATE TABLE entwuerfe (
    id              TEXT PRIMARY KEY,
    konto_id        TEXT NOT NULL REFERENCES konten(id) ON DELETE CASCADE,
    antwortet_auf   TEXT,
    betreff         TEXT NOT NULL DEFAULT '',
    an              TEXT NOT NULL DEFAULT '',
    kopie           TEXT NOT NULL DEFAULT '',
    blindkopie      TEXT NOT NULL DEFAULT '',
    koerper_html    TEXT NOT NULL DEFAULT '',
    geaendert_am    INTEGER NOT NULL
) STRICT;

-- Geschlummerte Gespräche: aus dem Eingang genommen bis `bis`. Rein lokal —
-- diese App hat keine Mitte, in der ein Zustand für mehrere Geräte liegen
-- könnte. Wer das später über Geräte hinweg will, braucht einen Ort dafür,
-- und das ist eine eigene Entscheidung.
CREATE TABLE schlummer (
    thread_schluessel TEXT PRIMARY KEY,
    konto_id        TEXT NOT NULL REFERENCES konten(id) ON DELETE CASCADE,
    bis             INTEGER NOT NULL,
    angelegt_am     INTEGER NOT NULL
) STRICT;

CREATE INDEX schlummer_faellig ON schlummer (bis);
