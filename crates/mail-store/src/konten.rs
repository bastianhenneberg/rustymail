//! Konten lesen und schreiben.
//!
//! Was hier NICHT durchgeht: das Geheimnis. Diese Tabelle kennt nur die
//! `tresor_kennung` — den Namen, unter dem Passwort oder Token im Tresor der
//! Plattform liegen. Eine SQLite-Datei im Benutzerverzeichnis ist für jedes
//! andere Programm desselben Benutzers lesbar; ein Postfachpasswort hat darin
//! nichts zu suchen.

use rusqlite::{OptionalExtension, params};

use crate::{Ergebnis, Speicher};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KontoZeile {
    pub id: String,
    pub beschriftung: String,
    pub adresse: String,
    /// `imap` oder `jmap`.
    pub protokoll: String,
    /// Host, Port, Benutzer, Verschlüsselung — als JSON, weil IMAP und JMAP
    /// verschiedene Felder brauchen und eine Spalte je Feld bei jedem neuen
    /// Protokoll eine Wanderung kostet.
    pub zugang_json: Option<String>,
    pub tresor_kennung: Option<String>,
    pub zuletzt_geprueft_am: Option<i64>,
    pub gesundheit: Option<String>,
}

impl Speicher {
    pub fn konten(&self) -> Ergebnis<Vec<KontoZeile>> {
        let mut abfrage = self.verbindung().prepare(
            "SELECT id, beschriftung, adresse, protokoll, zugang_json,
                    tresor_kennung, zuletzt_geprueft_am, gesundheit
             FROM konten ORDER BY beschriftung",
        )?;
        let zeilen = abfrage
            .query_map([], lese_zeile)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(zeilen)
    }

    pub fn konto(&self, id: &str) -> Ergebnis<Option<KontoZeile>> {
        let zeile = self
            .verbindung()
            .query_row(
                "SELECT id, beschriftung, adresse, protokoll, zugang_json,
                        tresor_kennung, zuletzt_geprueft_am, gesundheit
                 FROM konten WHERE id = ?1",
                [id],
                lese_zeile,
            )
            .optional()?;
        Ok(zeile)
    }

    /// Anlegen oder ersetzen. `INSERT OR REPLACE` mit Absicht: Ein zweiter
    /// Aufruf mit derselben Kennung soll nicht scheitern, sondern denselben
    /// Stand hinterlassen — dasselbe gilt für die Warteschlange später.
    pub fn konto_schreiben(&self, konto: &KontoZeile) -> Ergebnis<()> {
        self.verbindung().execute(
            "INSERT OR REPLACE INTO konten
                (id, beschriftung, adresse, protokoll, zugang_json,
                 tresor_kennung, zuletzt_geprueft_am, gesundheit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                konto.id,
                konto.beschriftung,
                konto.adresse,
                konto.protokoll,
                konto.zugang_json,
                konto.tresor_kennung,
                konto.zuletzt_geprueft_am,
                konto.gesundheit,
            ],
        )?;
        Ok(())
    }

    /// Löscht das Konto samt allem, was daran hängt — Ordner, Nachrichten,
    /// Entwürfe, Geschlummertes. Das übernehmen die Fremdschlüssel, aber nur
    /// weil `foreign_keys` eingeschaltet ist; ohne das Pragma bliebe hier
    /// alles liegen und tauchte beim nächsten Konto mit derselben Kennung
    /// wieder auf.
    pub fn konto_loeschen(&self, id: &str) -> Ergebnis<bool> {
        let betroffen = self
            .verbindung()
            .execute("DELETE FROM konten WHERE id = ?1", [id])?;
        Ok(betroffen > 0)
    }
}

fn lese_zeile(zeile: &rusqlite::Row<'_>) -> rusqlite::Result<KontoZeile> {
    Ok(KontoZeile {
        id: zeile.get(0)?,
        beschriftung: zeile.get(1)?,
        adresse: zeile.get(2)?,
        protokoll: zeile.get(3)?,
        zugang_json: zeile.get(4)?,
        tresor_kennung: zeile.get(5)?,
        zuletzt_geprueft_am: zeile.get(6)?,
        gesundheit: zeile.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zeile(id: &str) -> KontoZeile {
        KontoZeile {
            id: id.to_owned(),
            beschriftung: format!("Postfach {id}"),
            adresse: format!("{id}@beispiel.de"),
            protokoll: "imap".to_owned(),
            zugang_json: Some(r#"{"host":"mail.beispiel.de","port":993}"#.to_owned()),
            tresor_kennung: Some(format!("konto:{id}:zugang")),
            zuletzt_geprueft_am: None,
            gesundheit: None,
        }
    }

    #[test]
    fn schreiben_lesen_loeschen() {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        speicher.konto_schreiben(&zeile("a")).expect("schreiben");
        speicher.konto_schreiben(&zeile("b")).expect("schreiben");

        assert_eq!(speicher.konten().expect("lesen").len(), 2);
        assert_eq!(
            speicher.konto("a").expect("lesen").map(|k| k.adresse),
            Some("a@beispiel.de".to_owned())
        );

        assert!(speicher.konto_loeschen("a").expect("loeschen"));
        assert!(!speicher.konto_loeschen("a").expect("loeschen"), "zweites Löschen meldet false");
        assert_eq!(speicher.konten().expect("lesen").len(), 1);
    }

    #[test]
    fn zweites_schreiben_ersetzt_statt_zu_scheitern() {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        speicher.konto_schreiben(&zeile("a")).expect("schreiben");

        let mut geaendert = zeile("a");
        geaendert.beschriftung = "Umbenannt".to_owned();
        speicher.konto_schreiben(&geaendert).expect("zweites Schreiben");

        let konten = speicher.konten().expect("lesen");
        assert_eq!(konten.len(), 1, "kein zweiter Eintrag");
        assert_eq!(konten[0].beschriftung, "Umbenannt");
    }

    #[test]
    fn das_geheimnis_steht_nirgends_in_der_datei() {
        // Kein Test der Verschlüsselung, sondern des Datenmodells: Es gibt
        // keine Spalte, in die ein Passwort passen würde. Fällt diese Zusage,
        // schlägt der Test fehl, bevor jemand sie aus Bequemlichkeit bricht.
        let speicher = Speicher::fluechtig().expect("Datenbank");
        let spalten: Vec<String> = speicher
            .verbindung()
            .prepare("SELECT name FROM pragma_table_info('konten')")
            .expect("pragma")
            .query_map([], |z| z.get(0))
            .expect("abfrage")
            .collect::<rusqlite::Result<_>>()
            .expect("sammeln");

        for verboten in ["passwort", "password", "geheimnis", "token", "secret"] {
            assert!(
                !spalten.iter().any(|s| s.contains(verboten)),
                "Spalte „{verboten}“ gehört nicht in die Kontentabelle: {spalten:?}"
            );
        }
    }

    #[test]
    fn ein_geloeschtes_konto_nimmt_seine_nachrichten_mit() {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        speicher.konto_schreiben(&zeile("a")).expect("schreiben");
        speicher
            .verbindung()
            .execute(
                "INSERT INTO ordner (id, konto_id, pfad) VALUES ('o1', 'a', 'INBOX')",
                [],
            )
            .expect("ordner");
        speicher
            .verbindung()
            .execute(
                "INSERT INTO nachrichten
                    (id, konto_id, ordner_id, server_kennung, thread_schluessel, gesendet_am)
                 VALUES ('n1', 'a', 'o1', '1', 't1', 0)",
                [],
            )
            .expect("nachricht");

        speicher.konto_loeschen("a").expect("loeschen");

        let uebrig: i64 = speicher
            .verbindung()
            .query_row("SELECT count(*) FROM nachrichten", [], |z| z.get(0))
            .expect("zaehlen");
        assert_eq!(uebrig, 0, "Nachrichten eines gelöschten Kontos bleiben liegen");
    }
}
