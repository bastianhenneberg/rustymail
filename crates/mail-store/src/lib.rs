//! Der lokale Speicher. SQLite, offline-first.
//!
//! Die App muss im Flugzeug lesbar sein und im Zug Entwürfe annehmen. Deshalb
//! ist die Datenbank nicht ein Zwischenspeicher vor dem Server, sondern die
//! Quelle, aus der die Oberfläche IMMER liest; der Abgleich schreibt hinein.
//! Eine Oberfläche, die direkt am Netz hängt, hat keinen Zustand, den sie
//! anzeigen kann, wenn das Netz fehlt.

use std::path::Path;

use rusqlite::Connection;

mod konten;
mod nachrichten;
mod warteschlange;
pub use konten::KontoZeile;
pub use nachrichten::{Anhangzeile, Nachrichtendetail, Nachrichtenzeile, Threadzeile};
pub use warteschlange::{Auftrag, VERSUCHE_MAX};

/// Die Wanderungen, in Reihenfolge. Nach vorne wird angehängt, Bestehendes
/// nie geändert: Eine veränderte Wanderung läuft auf Geräten, die sie schon
/// angewandt haben, nicht erneut — dort entsteht dann ein Schema, das es
/// nirgends sonst gibt.
const WANDERUNGEN: &[(&str, &str)] = &[
    ("001_grundgeruest", include_str!("schema/001_grundgeruest.sql")),
    ("002_flaggen_spalten", include_str!("schema/002_flaggen_spalten.sql")),
];

#[derive(Debug, thiserror::Error)]
pub enum Speicherfehler {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Ergebnis<T> = Result<T, Speicherfehler>;

pub struct Speicher {
    verbindung: Connection,
}

impl Speicher {
    /// Öffnet die Datenbank unter `pfad` und wandert sie auf den aktuellen
    /// Stand. Der Pfad kommt von der Plattform — auf Mobile darf sich der
    /// Kern kein Verzeichnis aussuchen.
    pub fn oeffnen(pfad: &Path) -> Ergebnis<Self> {
        let verbindung = Connection::open(pfad)?;
        Self::einrichten(verbindung)
    }

    /// Im Speicher, für Tests.
    pub fn fluechtig() -> Ergebnis<Self> {
        let verbindung = Connection::open_in_memory()?;
        Self::einrichten(verbindung)
    }

    fn einrichten(verbindung: Connection) -> Ergebnis<Self> {
        // WAL: Lesen blockiert nicht, während der Abgleich schreibt. Ohne das
        // steht die Liste still, solange im Hintergrund eingetragen wird.
        verbindung.pragma_update(None, "journal_mode", "WAL")?;
        verbindung.pragma_update(None, "synchronous", "NORMAL")?;
        // Fremdschlüssel sind in SQLite je Verbindung abgeschaltet. Ohne
        // diese Zeile sind alle ON DELETE CASCADE oben Dekoration.
        verbindung.pragma_update(None, "foreign_keys", true)?;

        let mut speicher = Self { verbindung };
        speicher.wandern()?;
        Ok(speicher)
    }

    fn wandern(&mut self) -> Ergebnis<()> {
        self.verbindung.execute_batch(
            "CREATE TABLE IF NOT EXISTS wanderungen (
                name        TEXT PRIMARY KEY,
                gelaufen_am INTEGER NOT NULL
            ) STRICT;",
        )?;

        for (name, sql) in WANDERUNGEN {
            let schon: bool = self.verbindung.query_row(
                "SELECT EXISTS (SELECT 1 FROM wanderungen WHERE name = ?1)",
                [name],
                |zeile| zeile.get(0),
            )?;
            if schon {
                continue;
            }

            // Eine Wanderung ist ganz oder gar nicht gelaufen. Ohne die
            // Transaktion bleibt bei einem Fehler in der Mitte der Datei ein
            // halbes Schema zurück, und der nächste Start hält es für fertig.
            let zweig = self.verbindung.transaction()?;
            zweig.execute_batch(sql)?;
            zweig.execute(
                "INSERT INTO wanderungen (name, gelaufen_am) VALUES (?1, unixepoch() * 1000)",
                [name],
            )?;
            zweig.commit()?;
            tracing::info!(wanderung = name, "angewandt");
        }

        Ok(())
    }

    pub fn verbindung(&self) -> &Connection {
        &self.verbindung
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_entsteht_und_wandert_nur_einmal() {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        let tabellen: i64 = speicher
            .verbindung()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'nachrichten'",
                [],
                |z| z.get(0),
            )
            .expect("Abfrage");
        assert_eq!(tabellen, 1);

        // Ein zweiter Lauf darf nicht scheitern — sonst bricht jeder zweite
        // App-Start.
        let gelaufen: i64 = speicher
            .verbindung()
            .query_row("SELECT count(*) FROM wanderungen", [], |z| z.get(0))
            .expect("Abfrage");
        assert_eq!(gelaufen, WANDERUNGEN.len() as i64);
    }

    #[test]
    fn fremdschluessel_greifen() {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        // Ohne eingeschaltete Fremdschlüssel ginge das durch.
        let fehlschlag = speicher.verbindung().execute(
            "INSERT INTO ordner (id, konto_id, pfad) VALUES ('o1', 'gibtesnicht', 'INBOX')",
            [],
        );
        assert!(fehlschlag.is_err(), "Fremdschlüssel sind nicht aktiv");
    }
}
