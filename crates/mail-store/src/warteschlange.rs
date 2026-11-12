//! Was lokal passiert ist, aber noch nicht beim Server angekommen.
//!
//! Der Grund für diese Tabelle steht im Alltag: Jemand markiert im Zug drei
//! Mails als gelesen, das Netz ist weg, und beim nächsten Öffnen sind sie
//! wieder ungelesen. Ohne Warteschlange ist die lokale Änderung verloren,
//! sobald der Server das nächste Mal antwortet — der Abgleich überschreibt
//! sie mit dem Stand von dort.
//!
//! Jede Operation muss **idempotent wiederholbar** sein. Das Netz bricht
//! mitten im Senden weg, und beim nächsten Start weiss niemand, ob der
//! Server sie gesehen hat. „Als gelesen markieren" zweimal ist harmlos;
//! „eine Stelle weiter verschieben" nicht — deshalb trägt jede Operation ihr
//! Ziel und keine Richtung.

use rusqlite::params;

use crate::{Ergebnis, Speicher};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Auftrag {
    pub id: i64,
    pub konto_id: String,
    pub art: String,
    pub nutzlast_json: String,
    pub versuche: u32,
}

/// Nach wie vielen Fehlversuchen ein Auftrag liegen bleibt.
///
/// Nicht unbegrenzt: Ein Auftrag, der dauerhaft scheitert — gelöschte
/// Nachricht, entzogene Rechte —, würde sonst jeden Abgleich blockieren und
/// bei jedem Versuch eine Fehlermeldung erzeugen. Nach dem Deckel bleibt er
/// stehen und ist sichtbar, statt im Kreis zu laufen.
pub const VERSUCHE_MAX: u32 = 5;

impl Speicher {
    pub fn auftrag_anlegen(
        &self,
        konto_id: &str,
        art: &str,
        nutzlast_json: &str,
    ) -> Ergebnis<i64> {
        self.verbindung().execute(
            "INSERT INTO ausstehend (konto_id, art, nutzlast_json, angelegt_am)
             VALUES (?1, ?2, ?3, unixepoch() * 1000)",
            params![konto_id, art, nutzlast_json],
        )?;
        Ok(self.verbindung().last_insert_rowid())
    }

    /// Die nächsten offenen Aufträge, älteste zuerst.
    ///
    /// Die Reihenfolge ist Teil der Zusage: Wer eine Mail erst verschiebt und
    /// dann als gelesen markiert, darf nicht die umgekehrte Reihenfolge am
    /// Server sehen — die zweite Operation zeigt sonst auf einen Ordner, in
    /// dem die Nachricht nicht mehr liegt.
    pub fn auftraege(&self, konto_id: &str, anzahl: u32) -> Ergebnis<Vec<Auftrag>> {
        let mut abfrage = self.verbindung().prepare(
            "SELECT id, konto_id, art, nutzlast_json, versuche
             FROM ausstehend
             WHERE konto_id = ?1 AND versuche < ?2
             ORDER BY id ASC
             LIMIT ?3",
        )?;
        let zeilen = abfrage
            .query_map(params![konto_id, VERSUCHE_MAX, anzahl], |zeile| {
                Ok(Auftrag {
                    id: zeile.get(0)?,
                    konto_id: zeile.get(1)?,
                    art: zeile.get(2)?,
                    nutzlast_json: zeile.get(3)?,
                    versuche: zeile.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(zeilen)
    }

    pub fn auftrag_erledigt(&self, id: i64) -> Ergebnis<()> {
        self.verbindung()
            .execute("DELETE FROM ausstehend WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn auftrag_gescheitert(&self, id: i64, fehler: &str) -> Ergebnis<()> {
        self.verbindung().execute(
            "UPDATE ausstehend
             SET versuche = versuche + 1, letzter_fehler = ?2
             WHERE id = ?1",
            params![id, fehler],
        )?;
        Ok(())
    }

    /// Aufträge, die den Deckel erreicht haben und liegen bleiben.
    pub fn liegengebliebene(&self, konto_id: &str) -> Ergebnis<Vec<Auftrag>> {
        let mut abfrage = self.verbindung().prepare(
            "SELECT id, konto_id, art, nutzlast_json, versuche
             FROM ausstehend WHERE konto_id = ?1 AND versuche >= ?2 ORDER BY id ASC",
        )?;
        let zeilen = abfrage
            .query_map(params![konto_id, VERSUCHE_MAX], |zeile| {
                Ok(Auftrag {
                    id: zeile.get(0)?,
                    konto_id: zeile.get(1)?,
                    art: zeile.get(2)?,
                    nutzlast_json: zeile.get(3)?,
                    versuche: zeile.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(zeilen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KontoZeile;

    fn speicher() -> Speicher {
        let speicher = Speicher::fluechtig().expect("Datenbank");
        speicher
            .konto_schreiben(&KontoZeile {
                id: "k1".into(),
                beschriftung: "Test".into(),
                adresse: "test@beispiel.de".into(),
                protokoll: "imap".into(),
                zugang_json: None,
                tresor_kennung: None,
                zuletzt_geprueft_am: None,
                gesundheit: None,
            })
            .expect("Konto");
        speicher
    }

    #[test]
    fn auftraege_kommen_in_der_reihenfolge_zurueck_in_der_sie_entstanden() {
        let speicher = speicher();
        let erst = speicher.auftrag_anlegen("k1", "verschieben", "{}").expect("anlegen");
        let dann = speicher.auftrag_anlegen("k1", "flagge", "{}").expect("anlegen");

        let offen = speicher.auftraege("k1", 10).expect("lesen");
        assert_eq!(offen.iter().map(|a| a.id).collect::<Vec<_>>(), vec![erst, dann]);
    }

    #[test]
    fn erledigte_auftraege_verschwinden() {
        let speicher = speicher();
        let id = speicher.auftrag_anlegen("k1", "flagge", "{}").expect("anlegen");
        speicher.auftrag_erledigt(id).expect("erledigt");
        assert!(speicher.auftraege("k1", 10).expect("lesen").is_empty());
    }

    #[test]
    fn nach_zu_vielen_versuchen_bleibt_ein_auftrag_liegen() {
        let speicher = speicher();
        let id = speicher.auftrag_anlegen("k1", "flagge", "{}").expect("anlegen");

        for _ in 0..VERSUCHE_MAX {
            assert_eq!(speicher.auftraege("k1", 10).expect("lesen").len(), 1);
            speicher.auftrag_gescheitert(id, "nicht erreichbar").expect("gescheitert");
        }

        // Jetzt ist er aus dem Umlauf — aber nicht weg.
        assert!(
            speicher.auftraege("k1", 10).expect("lesen").is_empty(),
            "ein dauerhaft scheiternder Auftrag blockiert jeden Abgleich"
        );
        let liegen = speicher.liegengebliebene("k1").expect("lesen");
        assert_eq!(liegen.len(), 1);
        assert_eq!(liegen[0].versuche, VERSUCHE_MAX);
    }

    #[test]
    fn auftraege_fremder_konten_bleiben_unberuehrt() {
        let speicher = speicher();
        speicher
            .konto_schreiben(&KontoZeile {
                id: "k2".into(),
                beschriftung: "Zweit".into(),
                adresse: "zwei@beispiel.de".into(),
                protokoll: "imap".into(),
                zugang_json: None,
                tresor_kennung: None,
                zuletzt_geprueft_am: None,
                gesundheit: None,
            })
            .expect("Konto");

        speicher.auftrag_anlegen("k1", "flagge", "{}").expect("anlegen");
        speicher.auftrag_anlegen("k2", "flagge", "{}").expect("anlegen");

        assert_eq!(speicher.auftraege("k1", 10).expect("lesen").len(), 1);
        assert_eq!(speicher.auftraege("k2", 10).expect("lesen").len(), 1);
    }

    #[test]
    fn ein_geloeschtes_konto_nimmt_seine_auftraege_mit() {
        let speicher = speicher();
        speicher.auftrag_anlegen("k1", "flagge", "{}").expect("anlegen");
        speicher.konto_loeschen("k1").expect("löschen");
        assert!(speicher.auftraege("k1", 10).expect("lesen").is_empty());
    }
}
