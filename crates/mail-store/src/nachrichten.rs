//! Nachrichten schreiben und als Gespräche lesen.
//!
//! Die Oberfläche liest **immer** hier und **nie** aus dem Netz. Das ist
//! nicht Bequemlichkeit: Eine Liste, die am Netz hängt, hat nichts zu zeigen,
//! wenn das Netz fehlt — und sie zeigt beim Blättern Lücken, sobald jemand
//! anderes im Postfach etwas verschiebt.

use rusqlite::{OptionalExtension, params};

use crate::{Ergebnis, Speicher};

/// Eine Nachricht, wie sie aus den Kopfzeilen entsteht. Ohne Textkörper — der
/// kommt beim Öffnen nach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nachrichtenzeile {
    pub id: String,
    pub konto_id: String,
    pub ordner_id: String,
    pub server_kennung: String,
    pub message_id: Option<String>,
    pub thread_schluessel: String,
    pub betreff: String,
    /// Ohne Antwortpräfixe, kleingeschrieben — rechnet `mail-sync` aus.
    pub betreff_normalisiert: String,
    pub von_name: Option<String>,
    pub von_adresse: String,
    pub gesendet_am: i64,
    pub groesse: u32,
    pub flaggen: String,
    pub gelesen: bool,
    pub markiert: bool,
    pub hat_anhang: bool,
}

/// Eine Zeile der Gesprächsliste, vom SQL zusammengerechnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Threadzeile {
    pub thread_schluessel: String,
    pub betreff: String,
    pub beteiligte: Vec<String>,
    pub letzte_nachricht_am: i64,
    pub anzahl: u32,
    pub ungelesen: bool,
    pub markiert: bool,
    pub hat_anhang: bool,
}

impl Speicher {
    /// Legt einen Ordner an, falls er fehlt, und gibt seine Kennung zurück.
    ///
    /// Nachrichten hängen per Fremdschlüssel am Ordner; ohne diese Zeile
    /// schlägt jedes Einfügen fehl — mit einer Meldung, die auf die
    /// Nachricht zeigt und nicht auf den fehlenden Ordner.
    pub fn ordner_sichern(&self, id: &str, konto_id: &str, pfad: &str, trenner: char) -> Ergebnis<()> {
        self.verbindung().execute(
            "INSERT INTO ordner (id, konto_id, pfad, trenner)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET pfad = excluded.pfad",
            params![id, konto_id, pfad, trenner.to_string()],
        )?;
        Ok(())
    }

    /// Schreibt Nachrichten in einem Rutsch.
    ///
    /// Eine Transaktion für alle: 100 einzelne `INSERT` sind 100 fsync, und
    /// das ist der Unterschied zwischen einer Liste, die erscheint, und
    /// einer, die tröpfelt.
    pub fn nachrichten_schreiben(&mut self, zeilen: &[Nachrichtenzeile]) -> Ergebnis<()> {
        let zweig = self.verbindung.transaction()?;
        {
            let mut befehl = zweig.prepare(
                "INSERT INTO nachrichten
                    (id, konto_id, ordner_id, server_kennung, message_id, thread_schluessel,
                     betreff, betreff_normalisiert, von_name, von_adresse, gesendet_am,
                     groesse, flaggen, gelesen, markiert, hat_anhang)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT (ordner_id, server_kennung) DO UPDATE SET
                    message_id = excluded.message_id,
                    thread_schluessel = excluded.thread_schluessel,
                    betreff = excluded.betreff,
                    betreff_normalisiert = excluded.betreff_normalisiert,
                    von_name = excluded.von_name,
                    von_adresse = excluded.von_adresse,
                    gesendet_am = excluded.gesendet_am,
                    groesse = excluded.groesse,
                    flaggen = excluded.flaggen,
                    gelesen = excluded.gelesen,
                    markiert = excluded.markiert",
            )?;

            for zeile in zeilen {
                befehl.execute(params![
                    zeile.id,
                    zeile.konto_id,
                    zeile.ordner_id,
                    zeile.server_kennung,
                    zeile.message_id,
                    zeile.thread_schluessel,
                    zeile.betreff,
                    zeile.betreff_normalisiert,
                    zeile.von_name,
                    zeile.von_adresse,
                    zeile.gesendet_am,
                    zeile.groesse,
                    zeile.flaggen,
                    zeile.gelesen as i32,
                    zeile.markiert as i32,
                    zeile.hat_anhang as i32,
                ])?;
            }
        }
        zweig.commit()?;
        Ok(())
    }

    /// Der Gesprächsschlüssel einer bekannten Message-Id.
    pub fn thread_nach_message_id(&self, message_id: &str) -> Ergebnis<Option<String>> {
        let treffer = self
            .verbindung()
            .query_row(
                "SELECT thread_schluessel FROM nachrichten WHERE message_id = ?1 LIMIT 1",
                [message_id],
                |zeile| zeile.get(0),
            )
            .optional()?;
        Ok(treffer)
    }

    /// Der Gesprächsschlüssel zu einem normalisierten Betreff, innerhalb eines
    /// Zeitfensters.
    ///
    /// Das Fenster ist der Schutz: Ein Betreff wie „Angebot" taucht in zwei
    /// Jahren mehrfach auf, und ohne Grenze würde eine Antwort von heute an
    /// einem Vorgang von 2024 hängen.
    pub fn thread_nach_betreff(
        &self,
        normalisierter_betreff: &str,
        nicht_aelter_als: i64,
    ) -> Ergebnis<Option<String>> {
        let treffer = self
            .verbindung()
            .query_row(
                "SELECT thread_schluessel FROM nachrichten
                 WHERE betreff_normalisiert = ?1 AND gesendet_am >= ?2
                 ORDER BY gesendet_am DESC LIMIT 1",
                params![normalisierter_betreff, nicht_aelter_als],
                |zeile| zeile.get(0),
            )
            .optional()?;
        Ok(treffer)
    }

    /// Legt zwei Gespräche zu einem zusammen.
    pub fn threads_zusammenlegen(&self, von: &[String], nach: &str) -> Ergebnis<usize> {
        let mut betroffen = 0;
        for alt in von {
            betroffen += self.verbindung().execute(
                "UPDATE nachrichten SET thread_schluessel = ?1 WHERE thread_schluessel = ?2",
                params![nach, alt],
            )?;
        }
        Ok(betroffen)
    }

    /// Die Gesprächsliste eines Ordners, neueste zuerst.
    ///
    /// `vor_zeitpunkt` ist die Blättermarke: der Zeitpunkt der letzten Zeile
    /// der vorigen Seite. Kein OFFSET — zwischen zwei Seiten kann Post
    /// eintreffen, und dann überspringt OFFSET genau so viele Zeilen, wie
    /// dazugekommen sind.
    pub fn threads(
        &self,
        ordner_id: &str,
        vor_zeitpunkt: Option<i64>,
        anzahl: u32,
    ) -> Ergebnis<Vec<Threadzeile>> {
        let grenze = vor_zeitpunkt.unwrap_or(i64::MAX);

        let mut abfrage = self.verbindung().prepare(
            "SELECT thread_schluessel,
                    max(gesendet_am)                              AS letzte,
                    count(*)                                      AS anzahl,
                    max(CASE WHEN gelesen = 0 THEN 1 ELSE 0 END)  AS ungelesen,
                    max(markiert)                                 AS markiert,
                    max(hat_anhang)                               AS hat_anhang,
                    group_concat(DISTINCT von_adresse)            AS beteiligte
             FROM nachrichten
             WHERE ordner_id = ?1
             GROUP BY thread_schluessel
             HAVING letzte < ?2
             ORDER BY letzte DESC
             LIMIT ?3",
        )?;

        let zeilen = abfrage
            .query_map(params![ordner_id, grenze, anzahl], |zeile| {
                let schluessel: String = zeile.get(0)?;
                let beteiligte: Option<String> = zeile.get(6)?;
                Ok((
                    schluessel,
                    zeile.get::<_, i64>(1)?,
                    zeile.get::<_, u32>(2)?,
                    zeile.get::<_, i32>(3)? == 1,
                    zeile.get::<_, i32>(4)? == 1,
                    zeile.get::<_, i32>(5)? == 1,
                    beteiligte.unwrap_or_default(),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Der Betreff kommt aus der NEUESTEN Nachricht des Gesprächs, nicht
        // aus der ältesten: Wird ein Vorgang umbenannt („Re: Angebot" →
        // „Auftrag 1147"), soll die Liste den neuen Namen zeigen.
        let mut liste = Vec::with_capacity(zeilen.len());
        for (schluessel, letzte, anzahl, ungelesen, markiert, hat_anhang, beteiligte) in zeilen {
            let betreff: String = self
                .verbindung()
                .query_row(
                    "SELECT betreff FROM nachrichten
                     WHERE ordner_id = ?1 AND thread_schluessel = ?2
                     ORDER BY gesendet_am DESC LIMIT 1",
                    params![ordner_id, &schluessel],
                    |zeile| zeile.get(0),
                )
                .optional()?
                .unwrap_or_default();

            liste.push(Threadzeile {
                thread_schluessel: schluessel,
                betreff,
                beteiligte: beteiligte
                    .split(',')
                    .filter(|wert| !wert.is_empty())
                    .map(str::to_owned)
                    .collect(),
                letzte_nachricht_am: letzte,
                anzahl,
                ungelesen,
                markiert,
                hat_anhang,
            });
        }

        Ok(liste)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KontoZeile;

    fn speicher_mit_ordner() -> Speicher {
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
        speicher.ordner_sichern("k1/INBOX", "k1", "INBOX", '/').expect("Ordner");
        speicher
    }

    fn zeile(kennung: &str, thread: &str, betreff: &str, zeitpunkt: i64) -> Nachrichtenzeile {
        Nachrichtenzeile {
            id: format!("k1/INBOX/{kennung}"),
            konto_id: "k1".into(),
            ordner_id: "k1/INBOX".into(),
            server_kennung: kennung.into(),
            message_id: Some(format!("{kennung}@beispiel.de")),
            thread_schluessel: thread.into(),
            betreff: betreff.into(),
            betreff_normalisiert: betreff.to_lowercase(),
            von_name: None,
            von_adresse: format!("{kennung}@absender.de"),
            gesendet_am: zeitpunkt,
            groesse: 100,
            flaggen: String::new(),
            gelesen: false,
            markiert: false,
            hat_anhang: false,
        }
    }

    #[test]
    fn nachrichten_werden_zu_gespraechen() {
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[
                zeile("1", "t1", "Angebot", 1_000),
                zeile("2", "t1", "Re: Angebot", 2_000),
                zeile("3", "t2", "Rechnung", 1_500),
            ])
            .expect("schreiben");

        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");

        assert_eq!(threads.len(), 2, "aus drei Nachrichten werden zwei Gespräche");
        // Neueste zuerst.
        assert_eq!(threads[0].thread_schluessel, "t1");
        assert_eq!(threads[0].anzahl, 2);
        assert_eq!(threads[0].letzte_nachricht_am, 2_000);
        // Der Betreff kommt aus der NEUESTEN Nachricht.
        assert_eq!(threads[0].betreff, "Re: Angebot");
        // Beide Absender erscheinen.
        assert_eq!(threads[0].beteiligte.len(), 2);
    }

    #[test]
    fn ungelesen_markiert_und_anhang_schlagen_aufs_gespraech_durch() {
        let mut speicher = speicher_mit_ordner();
        let mut gelesen = zeile("1", "t1", "Angebot", 1_000);
        gelesen.gelesen = true;
        let mut zweite = zeile("2", "t1", "Re: Angebot", 2_000);
        zweite.gelesen = true;
        zweite.markiert = true;
        zweite.hat_anhang = true;

        speicher.nachrichten_schreiben(&[gelesen, zweite]).expect("schreiben");
        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");
        assert!(!threads[0].ungelesen, "alles gelesen, trotzdem ungelesen gemeldet");
        assert!(threads[0].markiert);
        assert!(threads[0].hat_anhang);

        // Eine ungelesene Nachricht macht das ganze Gespräch ungelesen — so
        // sucht man es in der Liste auch.
        let mut dritte = zeile("3", "t1", "Re: Angebot", 3_000);
        dritte.gelesen = false;
        speicher.nachrichten_schreiben(&[dritte]).expect("schreiben");
        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");
        assert!(threads[0].ungelesen);
    }

    #[test]
    fn zweimal_schreiben_legt_nichts_doppelt_an_und_zieht_flaggen_nach() {
        let mut speicher = speicher_mit_ordner();
        speicher.nachrichten_schreiben(&[zeile("1", "t1", "Angebot", 1_000)]).expect("erst");

        let mut erneut = zeile("1", "t1", "Angebot", 1_000);
        erneut.gelesen = true;
        speicher.nachrichten_schreiben(&[erneut]).expect("erneut");

        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].anzahl, 1, "die Nachricht liegt doppelt in der Tabelle");
        assert!(!threads[0].ungelesen, "die Flagge wurde nicht nachgezogen");
    }

    #[test]
    fn dieselbe_uid_mit_anderem_inhalt_wird_ueberschrieben() {
        // Der Fall, der am 12.11.2026 echte Daten verfälscht hat: Nach einem
        // Neuaufbau des Postfachs auf dem Server zeigt UID 3 auf eine ANDERE
        // Nachricht. Wer beim Konflikt nur die Flaggen nachzieht, behält den
        // alten Betreff unter der neuen Kennung — und die echte Nachricht
        // fehlt, ohne dass irgendwo ein Fehler auftaucht.
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[zeile("3", "t-alt", "Rechnung", 1_000)])
            .expect("erst");

        let mut neu = zeile("3", "t-neu", "Notiz an mich", 2_000);
        neu.message_id = Some("vierte@beispiel.de".into());
        speicher.nachrichten_schreiben(&[neu]).expect("erneut");

        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");
        assert_eq!(threads.len(), 1, "es darf nur eine Nachricht geben");
        assert_eq!(threads[0].betreff, "Notiz an mich", "der alte Betreff blieb stehen");
        assert_eq!(
            speicher.thread_nach_message_id("vierte@beispiel.de").expect("suchen"),
            Some("t-neu".to_owned()),
            "die Kennung wurde nicht nachgezogen"
        );
    }

    #[test]
    fn eine_geaenderte_uidvalidity_leert_den_ordner() {
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[
                zeile("1", "t1", "Angebot", 1_000),
                zeile("2", "t2", "Rechnung", 2_000),
            ])
            .expect("schreiben");
        speicher.uidvalidity_merken("k1/INBOX", 111).expect("merken");
        assert_eq!(speicher.uidvalidity("k1/INBOX").expect("lesen"), Some(111));

        let geloescht = speicher.ordner_zuruecksetzen("k1/INBOX", 222).expect("zurücksetzen");

        assert_eq!(geloescht, 2);
        assert!(speicher.threads("k1/INBOX", None, 10).expect("lesen").is_empty());
        assert_eq!(speicher.uidvalidity("k1/INBOX").expect("lesen"), Some(222));
    }

    #[test]
    fn blaettern_nach_zeitpunkt_ueberspringt_nichts() {
        let mut speicher = speicher_mit_ordner();
        let zeilen: Vec<_> = (1..=5)
            .map(|nummer| {
                zeile(
                    &nummer.to_string(),
                    &format!("t{nummer}"),
                    "Betreff",
                    nummer as i64 * 1_000,
                )
            })
            .collect();
        speicher.nachrichten_schreiben(&zeilen).expect("schreiben");

        let erste = speicher.threads("k1/INBOX", None, 2).expect("Seite 1");
        assert_eq!(erste.len(), 2);
        let marke = erste.last().expect("letzte").letzte_nachricht_am;

        let zweite = speicher.threads("k1/INBOX", Some(marke), 2).expect("Seite 2");
        assert_eq!(zweite.len(), 2);

        let erste_schluessel: Vec<&str> =
            erste.iter().map(|z| z.thread_schluessel.as_str()).collect();
        for zeile in &zweite {
            assert!(
                !erste_schluessel.contains(&zeile.thread_schluessel.as_str()),
                "{} steht auf beiden Seiten",
                zeile.thread_schluessel
            );
        }
    }

    #[test]
    fn zusammenlegen_macht_aus_zwei_gespraechen_eins() {
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[
                zeile("1", "t1", "Angebot", 1_000),
                zeile("2", "t2", "Rechnung", 2_000),
            ])
            .expect("schreiben");
        assert_eq!(speicher.threads("k1/INBOX", None, 10).expect("lesen").len(), 2);

        let betroffen = speicher
            .threads_zusammenlegen(&["t2".to_owned()], "t1")
            .expect("zusammenlegen");
        assert_eq!(betroffen, 1);

        let threads = speicher.threads("k1/INBOX", None, 10).expect("lesen");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].anzahl, 2);
    }

    #[test]
    fn der_betreff_rueckfall_schaut_nicht_in_die_vorvergangenheit() {
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[zeile("1", "t1", "Angebot", 1_000)])
            .expect("schreiben");

        // Innerhalb des Fensters: gefunden.
        assert_eq!(
            speicher.thread_nach_betreff("angebot", 500).expect("suchen"),
            Some("t1".to_owned())
        );
        // Ausserhalb: nicht gefunden. Ohne diese Grenze hängt eine Antwort
        // von heute am gleichnamigen Vorgang von vorletztem Jahr.
        assert_eq!(speicher.thread_nach_betreff("angebot", 2_000).expect("suchen"), None);
    }

    #[test]
    fn message_id_findet_ihr_gespraech() {
        let mut speicher = speicher_mit_ordner();
        speicher
            .nachrichten_schreiben(&[zeile("1", "t1", "Angebot", 1_000)])
            .expect("schreiben");

        assert_eq!(
            speicher.thread_nach_message_id("1@beispiel.de").expect("suchen"),
            Some("t1".to_owned())
        );
        assert_eq!(speicher.thread_nach_message_id("gibtesnicht").expect("suchen"), None);
    }
}

/// Eine Nachricht mit allem, was die Anzeige braucht.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nachrichtendetail {
    pub id: String,
    pub server_kennung: String,
    pub message_id: Option<String>,
    pub betreff: String,
    pub von_adresse: String,
    pub gesendet_am: i64,
    pub gelesen: bool,
    pub koerper_geholt: bool,
    pub text: Option<String>,
    pub html: Option<String>,
}

impl Speicher {
    /// Alle Nachrichten eines Gesprächs, älteste zuerst — so liest man einen
    /// Verlauf.
    pub fn nachrichten_im_thread(
        &self,
        ordner_id: &str,
        thread_schluessel: &str,
    ) -> Ergebnis<Vec<Nachrichtendetail>> {
        let mut abfrage = self.verbindung().prepare(
            "SELECT n.id, n.server_kennung, n.betreff, n.von_adresse, n.gesendet_am,
                    n.gelesen, n.koerper_geholt, k.text, k.html, n.message_id
             FROM nachrichten n
             LEFT JOIN koerper k ON k.nachricht_id = n.id
             WHERE n.ordner_id = ?1 AND n.thread_schluessel = ?2
             ORDER BY n.gesendet_am ASC",
        )?;

        let zeilen = abfrage
            .query_map(params![ordner_id, thread_schluessel], |zeile| {
                Ok(Nachrichtendetail {
                    id: zeile.get(0)?,
                    server_kennung: zeile.get(1)?,
                    message_id: zeile.get(9)?,
                    betreff: zeile.get(2)?,
                    von_adresse: zeile.get(3)?,
                    gesendet_am: zeile.get(4)?,
                    gelesen: zeile.get::<_, i32>(5)? == 1,
                    koerper_geholt: zeile.get::<_, i32>(6)? == 1,
                    text: zeile.get(7)?,
                    html: zeile.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(zeilen)
    }

    /// Legt den Textkörper ab und merkt am Kopf, dass er da ist.
    ///
    /// Die Rohfassung wandert mit in die Ablage: Sie ist die einzige Quelle,
    /// aus der sich später etwas gewinnen lässt, das heute noch niemand
    /// braucht — eine Signaturprüfung, ein fehlender Anhang, ein Kopfzeilen-
    /// Feld. Ein zweites Mal holen kann man sie nicht, wenn die Nachricht auf
    /// dem Server gelöscht wurde.
    pub fn koerper_schreiben(
        &mut self,
        nachricht_id: &str,
        text: Option<&str>,
        html: Option<&str>,
        rohdaten: &[u8],
    ) -> Ergebnis<()> {
        let zweig = self.verbindung.transaction()?;
        zweig.execute(
            "INSERT INTO koerper (nachricht_id, text, html, roh)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (nachricht_id) DO UPDATE SET
                text = excluded.text, html = excluded.html, roh = excluded.roh",
            params![nachricht_id, text, html, rohdaten],
        )?;
        zweig.execute(
            "UPDATE nachrichten SET koerper_geholt = 1 WHERE id = ?1",
            [nachricht_id],
        )?;
        zweig.commit()?;
        Ok(())
    }

    /// Anhänge einer Nachricht ablegen. Ersetzt, was vorher da war — eine
    /// Nachricht ändert sich nicht, aber ein wiederholtes Zerlegen soll nicht
    /// doppelte Zeilen hinterlassen.
    pub fn anhaenge_schreiben(&mut self, nachricht_id: &str, anhaenge: &[Anhangzeile]) -> Ergebnis<()> {
        let zweig = self.verbindung.transaction()?;
        zweig.execute("DELETE FROM anhaenge WHERE nachricht_id = ?1", [nachricht_id])?;
        {
            let mut befehl = zweig.prepare(
                "INSERT INTO anhaenge
                    (id, nachricht_id, dateiname, medientyp, groesse, content_id, inline)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for anhang in anhaenge {
                befehl.execute(params![
                    anhang.id,
                    nachricht_id,
                    anhang.dateiname,
                    anhang.medientyp,
                    anhang.groesse,
                    anhang.content_id,
                    anhang.inline as i32,
                ])?;
            }
        }
        zweig.commit()?;
        Ok(())
    }

    pub fn anhaenge(&self, nachricht_id: &str) -> Ergebnis<Vec<Anhangzeile>> {
        let mut abfrage = self.verbindung().prepare(
            "SELECT id, dateiname, medientyp, groesse, content_id, inline
             FROM anhaenge WHERE nachricht_id = ?1 ORDER BY dateiname",
        )?;
        let zeilen = abfrage
            .query_map([nachricht_id], |zeile| {
                Ok(Anhangzeile {
                    id: zeile.get(0)?,
                    dateiname: zeile.get(1)?,
                    medientyp: zeile.get(2)?,
                    groesse: zeile.get(3)?,
                    content_id: zeile.get(4)?,
                    inline: zeile.get::<_, i32>(5)? == 1,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(zeilen)
    }

    /// Die gespeicherte UIDVALIDITY eines Ordners.
    pub fn uidvalidity(&self, ordner_id: &str) -> Ergebnis<Option<u32>> {
        let wert = self
            .verbindung()
            .query_row(
                "SELECT uidvalidity FROM ordner WHERE id = ?1",
                [ordner_id],
                |zeile| zeile.get::<_, Option<u32>>(0),
            )
            .optional()?
            .flatten();
        Ok(wert)
    }

    /// Wirft alles weg, was zu einem Ordner gespeichert ist, und merkt sich
    /// die neue UIDVALIDITY.
    ///
    /// Das ist die einzige richtige Antwort auf eine geänderte UIDVALIDITY:
    /// Jede gespeicherte UID zeigt danach auf eine ANDERE Nachricht — oder
    /// auf gar keine. Wer stattdessen weiterschreibt, behält alte Betreffs
    /// unter neuen Kennungen, und die Liste zeigt Nachrichten, die es so nie
    /// gab. Auslöser sind Serverumzüge, Wiederherstellungen aus einer
    /// Sicherung und bei manchen Anbietern ein Tarifwechsel.
    pub fn ordner_zuruecksetzen(&self, ordner_id: &str, uidvalidity: u32) -> Ergebnis<usize> {
        let geloescht = self
            .verbindung()
            .execute("DELETE FROM nachrichten WHERE ordner_id = ?1", [ordner_id])?;
        self.verbindung().execute(
            "UPDATE ordner SET uidvalidity = ?2, highestmodseq = NULL WHERE id = ?1",
            params![ordner_id, uidvalidity],
        )?;
        Ok(geloescht)
    }

    pub fn uidvalidity_merken(&self, ordner_id: &str, uidvalidity: u32) -> Ergebnis<()> {
        self.verbindung().execute(
            "UPDATE ordner SET uidvalidity = ?2 WHERE id = ?1",
            params![ordner_id, uidvalidity],
        )?;
        Ok(())
    }

    /// Setzt die Gelesen-Flagge lokal. Der Server erfährt es über die
    /// Warteschlange — die Oberfläche darf nicht darauf warten.
    pub fn gelesen_merken(&self, nachricht_id: &str, gelesen: bool) -> Ergebnis<()> {
        self.verbindung().execute(
            "UPDATE nachrichten SET gelesen = ?2 WHERE id = ?1",
            params![nachricht_id, gelesen as i32],
        )?;
        Ok(())
    }

    pub fn markiert_merken(&self, nachricht_id: &str, markiert: bool) -> Ergebnis<()> {
        self.verbindung().execute(
            "UPDATE nachrichten SET markiert = ?2 WHERE id = ?1",
            params![nachricht_id, markiert as i32],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anhangzeile {
    pub id: String,
    pub dateiname: String,
    pub medientyp: String,
    pub groesse: u32,
    pub content_id: Option<String>,
    pub inline: bool,
}
