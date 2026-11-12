//! Die Fassade. Alles, was die Oberflächen aufrufen, steht hier.
//!
//! Regeln für diese Datei:
//!
//! 1. **Befehle rein, Ansichten raus.** Keine Zugriffsfunktionen auf einzelne
//!    Felder, keine Handles auf innere Objekte. Ein Kern, der `betreff(id)`
//!    einzeln anbietet, wird von der Liste pro Zeile mehrfach gerufen.
//! 2. **Nichts Blockierendes.** Jede Methode, die Netz oder Platte berührt,
//!    ist `async`; UniFFI macht daraus Swift-`async` und Kotlin-`suspend`.
//! 3. **Keine Panik.** Fehler werden zurückgegeben. Eine Panik über die
//!    FFI-Grenze ist undefiniertes Verhalten, über JNI bringt sie die
//!    Laufzeitumgebung um — auf dem Gerät sieht beides aus wie ein Absturz
//!    ohne Ursache.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use mail_proto::{Kontobackend, Zugang};
use mail_store::{KontoZeile, Speicher};

use crate::abgleich;
use crate::antwort::{self, Antwortentwurf};
use crate::ansicht::Adresse;
use crate::koerper;
use crate::ansicht::{
    Gesundheit, KontoAnsicht, NachrichtAnsicht, Ordnerrolle, OrdnerAnsicht, ThreadSeite,
    ThreadZeile,
};
use crate::ereignis::EreignisHoerer;
use crate::fehler::{Ergebnis, Fehler};
use crate::konto::{Kontoentwurf, Protokoll, Verbindungsdaten, tresor_kennung};
use crate::tresor::Tresor;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Einrichtung {
    /// Wohin die lokale Datenbank gehört. Auf Mobile das von der Plattform
    /// zugewiesene Verzeichnis — der Kern darf sich keines aussuchen.
    pub datenverzeichnis: String,
}

#[cfg_attr(feature = "ffi", derive(uniffi::Object))]
pub struct Kern {
    speicher: Mutex<Speicher>,
    tresor: Arc<dyn Tresor>,
    #[allow(dead_code)]
    hoerer: Arc<dyn EreignisHoerer>,
}

#[cfg_attr(feature = "ffi", uniffi::export(async_runtime = "tokio"))]
impl Kern {
    #[cfg_attr(feature = "ffi", uniffi::constructor)]
    pub fn neu(
        einrichtung: Einrichtung,
        tresor: Arc<dyn Tresor>,
        hoerer: Arc<dyn EreignisHoerer>,
    ) -> Ergebnis<Arc<Self>> {
        let verzeichnis = PathBuf::from(&einrichtung.datenverzeichnis);
        std::fs::create_dir_all(&verzeichnis).map_err(|fehler| Fehler::Speicher {
            grund: format!("{} ist nicht anlegbar: {fehler}", verzeichnis.display()),
        })?;

        let speicher = Speicher::oeffnen(&verzeichnis.join("postfach.sqlite3"))
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        Ok(Arc::new(Self {
            speicher: Mutex::new(speicher),
            tresor,
            hoerer,
        }))
    }

    pub async fn konten(&self) -> Ergebnis<Vec<KontoAnsicht>> {
        let zeilen = self
            .speicher()?
            .konten()
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        Ok(zeilen.into_iter().map(als_ansicht).collect())
    }

    /// Legt ein Konto an und gibt seine Kennung zurück.
    ///
    /// Die Reihenfolge ist Absicht: **erst der Tresor, dann die Zeile.**
    /// Scheitert der Tresor, gibt es kein Konto, das auf ein Geheimnis zeigt,
    /// welches nicht existiert — die Oberfläche zeigte sonst ein Postfach,
    /// das sich bei jedem Verbindungsversuch wortlos weigert. Scheitert
    /// umgekehrt die Zeile, wird der Tresoreintrag wieder entfernt; bleibt er
    /// liegen, sammelt sich im Schlüsselbund des Benutzers Müll an, den
    /// niemand mehr zuordnen kann.
    pub async fn konto_anlegen(&self, entwurf: Kontoentwurf) -> Ergebnis<String> {
        if entwurf.adresse.trim().is_empty() {
            return Err(Fehler::NichtGefunden { was: "eine Adresse".into() });
        }

        let id = uuid::Uuid::new_v4().to_string();
        let kennung = tresor_kennung(&id);

        self.tresor.schreib(kennung.clone(), entwurf.geheimnis.clone())?;

        let zugang_json = serde_json::to_string(&entwurf.verbindung).map_err(|fehler| {
            Fehler::Speicher { grund: format!("Verbindungsdaten nicht schreibbar: {fehler}") }
        })?;

        let zeile = KontoZeile {
            id: id.clone(),
            beschriftung: if entwurf.beschriftung.trim().is_empty() {
                entwurf.adresse.clone()
            } else {
                entwurf.beschriftung.clone()
            },
            adresse: entwurf.adresse.clone(),
            protokoll: entwurf.protokoll.als_text().to_owned(),
            zugang_json: Some(zugang_json),
            tresor_kennung: Some(kennung.clone()),
            zuletzt_geprueft_am: None,
            gesundheit: None,
        };

        let geschrieben = self
            .speicher()
            .and_then(|speicher| {
                speicher
                    .konto_schreiben(&zeile)
                    .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })
            });

        if let Err(fehler) = geschrieben {
            let _ = self.tresor.loesche(kennung);
            return Err(fehler);
        }

        Ok(id)
    }

    /// Entfernt das Konto, alles daran Hängende und den Tresoreintrag.
    pub async fn konto_entfernen(&self, konto_id: String) -> Ergebnis<()> {
        let kennung = self
            .speicher()?
            .konto(&konto_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?
            .and_then(|zeile| zeile.tresor_kennung)
            .unwrap_or_else(|| tresor_kennung(&konto_id));

        let geloescht = self
            .speicher()?
            .konto_loeschen(&konto_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        if !geloescht {
            return Err(Fehler::NichtGefunden { was: format!("Konto {konto_id}") });
        }

        // Der Tresoreintrag wird NACH der Zeile entfernt und sein Fehlschlag
        // nicht weitergegeben: Das Konto ist weg, und ein Geheimnis ohne
        // Konto ist unschön, aber nicht gefährlich. Die andere Reihenfolge
        // könnte ein Konto zurücklassen, dessen Geheimnis fehlt.
        if let Err(fehler) = self.tresor.loesche(kennung) {
            tracing::warn!(%konto_id, %fehler, "Tresoreintrag blieb liegen");
        }

        Ok(())
    }

    pub async fn ordner(&self, konto_id: String) -> Ergebnis<Vec<OrdnerAnsicht>> {
        let backend = self.backend(&konto_id)?;
        let ordner = backend.ordner().await?;

        Ok(ordner
            .into_iter()
            .map(|ordner| OrdnerAnsicht {
                // Die Kennung trägt das Konto mit: Zwei Konten haben beide
                // eine INBOX, und die Oberfläche hält nur diese Zeichenkette
                // in der Hand.
                id: format!("{konto_id}/{}", ordner.pfad),
                beschriftung: ordner.name.clone(),
                rolle: rolle(&ordner),
                pfad: ordner.pfad,
                gesamt: ordner.gesamt,
                ungelesen: ordner.ungelesen,
            })
            .collect())
    }

    /// Eine Seite Gesprächszeilen. `marke` ist `None` für die erste Seite und
    /// sonst die `weitere_marke` der vorigen. Keine Seitenzahlen: Ein
    /// IMAP-Ordner verändert sich unter der Hand, eine Marke überlebt das.
    /// Die Gesprächsliste — **nur aus der Datenbank**. Wer frische Post
    /// will, ruft vorher `abgleichen`.
    pub async fn threads(
        &self,
        ordner_id: String,
        marke: Option<String>,
        anzahl: u32,
    ) -> Ergebnis<ThreadSeite> {
        let vor_zeitpunkt = match marke.as_deref() {
            Some(wert) => Some(wert.parse::<i64>().map_err(|_| Fehler::NichtGefunden {
                was: format!("Blättermarke „{wert}“"),
            })?),
            None => None,
        };

        let zeilen = self
            .speicher()?
            .threads(&ordner_id, vor_zeitpunkt, anzahl.max(1))
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        // Die Marke ist der Zeitpunkt der letzten Zeile. Nur setzen, wenn die
        // Seite voll war — sonst blättert die Oberfläche ins Leere und zeigt
        // einen Mehr-Knopf, hinter dem nichts ist.
        let weitere_marke = (zeilen.len() as u32 >= anzahl.max(1))
            .then(|| zeilen.last().map(|zeile| zeile.letzte_nachricht_am.to_string()))
            .flatten();

        Ok(ThreadSeite {
            zeilen: zeilen
                .into_iter()
                .map(|zeile| ThreadZeile {
                    thread_id: zeile.thread_schluessel,
                    betreff: if zeile.betreff.trim().is_empty() {
                        "(ohne Betreff)".to_owned()
                    } else {
                        zeile.betreff
                    },
                    beteiligte: zeile.beteiligte,
                    // Kommt mit dem Textkörper nach.
                    vorschau: String::new(),
                    letzte_nachricht_am: zeile.letzte_nachricht_am,
                    anzahl: zeile.anzahl,
                    ungelesen: zeile.ungelesen,
                    markiert: zeile.markiert,
                    hat_anhang: zeile.hat_anhang,
                    schlummert_bis: None,
                })
                .collect(),
            weitere_marke,
        })
    }

    /// Alle Nachrichten eines Gesprächs, älteste zuerst.
    ///
    /// Fehlende Textkörper werden einmal geholt und abgelegt. Das zweite
    /// Öffnen geht damit ohne Netz — und ohne die Wartezeit, die eine Mail
    /// über eine Mobilverbindung sonst jedes Mal kostet.
    pub async fn gespraech(
        &self,
        ordner_id: String,
        thread_id: String,
    ) -> Ergebnis<Vec<NachrichtAnsicht>> {
        let (konto_id, pfad) = zerlege(&ordner_id)?;
        tracing::debug!(%ordner_id, %thread_id, "Gespräch wird geholt");

        let zeilen = self
            .speicher()?
            .nachrichten_im_thread(&ordner_id, &thread_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        // Erst sammeln, WAS fehlt, dann holen. Die Sperre auf dem Speicher
        // darf nicht über einen Netzzugriff gehalten werden — sonst steht
        // die ganze Oberfläche, solange ein Server trödelt.
        let fehlende: Vec<(String, String)> = zeilen
            .iter()
            .filter(|zeile| !zeile.koerper_geholt)
            .map(|zeile| (zeile.id.clone(), zeile.server_kennung.clone()))
            .collect();

        if !fehlende.is_empty() {
            tracing::debug!(anzahl = fehlende.len(), "Körper fehlen, Backend wird gebaut");
            let backend = self.backend(&konto_id)?;
            tracing::debug!("Backend steht");
            for (nachricht_id, kennung) in fehlende {
                tracing::debug!(%kennung, "Rohfassung wird geholt");
                let rohdaten = backend.rohfassung(&pfad, &kennung).await?;
                tracing::debug!(%kennung, bytes = rohdaten.len(), "Rohfassung da");
                let Some(zerlegung) = koerper::zerlegen(&rohdaten, &nachricht_id) else {
                    tracing::warn!(%nachricht_id, "Rohfassung nicht zerlegbar");
                    continue;
                };

                let mut speicher = self.speicher()?;
                speicher
                    .koerper_schreiben(
                        &nachricht_id,
                        zerlegung.text.as_deref(),
                        zerlegung.html.as_deref(),
                        &rohdaten,
                    )
                    .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
                speicher
                    .anhaenge_schreiben(
                        &nachricht_id,
                        &zerlegung
                            .anhaenge
                            .iter()
                            .map(|anhang| mail_store::Anhangzeile {
                                id: anhang.id.clone(),
                                dateiname: anhang.dateiname.clone(),
                                medientyp: anhang.medientyp.clone(),
                                groesse: anhang.groesse as u32,
                                content_id: anhang.content_id.clone(),
                                inline: anhang.inline,
                            })
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
            }
        }

        // Noch einmal lesen — jetzt mit Körpern.
        let speicher = self.speicher()?;
        let zeilen = speicher
            .nachrichten_im_thread(&ordner_id, &thread_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        let mut ansichten = Vec::with_capacity(zeilen.len());
        for zeile in zeilen {
            let anhaenge = speicher
                .anhaenge(&zeile.id)
                .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

            ansichten.push(NachrichtAnsicht {
                id: zeile.id,
                thread_id: thread_id.clone(),
                betreff: zeile.betreff,
                von: Adresse { name: None, adresse: zeile.von_adresse },
                // Empfänger kommen mit der Rohfassung und werden noch nicht
                // abgelegt — dafür braucht es eine eigene Spalte.
                an: Vec::new(),
                kopie: Vec::new(),
                blindkopie: Vec::new(),
                gesendet_am: zeile.gesendet_am,
                koerper: koerper::als_koerper(zeile.text, zeile.html),
                anhaenge: anhaenge
                    .into_iter()
                    // Inline-Bilder stehen im Text, nicht in der Liste
                    // darunter.
                    .filter(|anhang| !anhang.inline)
                    .map(|anhang| crate::ansicht::AnhangAnsicht {
                        id: anhang.id,
                        dateiname: anhang.dateiname,
                        medientyp: anhang.medientyp,
                        groesse: anhang.groesse as u64,
                        inline: anhang.inline,
                        content_id: anhang.content_id,
                    })
                    .collect(),
            });
        }

        Ok(ansichten)
    }

    /// Antwortet auf die neueste Nachricht eines Gesprächs.
    ///
    /// Reihenfolge mit Bedacht: **erst senden, dann die Kopie ablegen.**
    /// Scheitert die Kopie, ist die Antwort trotzdem unterwegs — und genau
    /// das wird gemeldet, statt den ganzen Vorgang als Fehler auszugeben.
    /// Ein Client, der einen geglückten Versand als Fehlschlag meldet,
    /// bringt Leute dazu, ein zweites Mal zu senden.
    pub async fn antworten(
        &self,
        ordner_id: String,
        thread_id: String,
        text: String,
    ) -> Ergebnis<()> {
        let (konto_id, _pfad) = zerlege(&ordner_id)?;

        let (konto, bezug) = {
            let speicher = self.speicher()?;
            let konto = speicher
                .konto(&konto_id)
                .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?
                .ok_or_else(|| Fehler::NichtGefunden { was: format!("Konto {konto_id}") })?;
            let bezug = speicher
                .nachrichten_im_thread(&ordner_id, &thread_id)
                .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?
                .pop()
                .ok_or_else(|| Fehler::NichtGefunden {
                    was: format!("Gespräch {thread_id}"),
                })?;
            (konto, bezug)
        };

        let jetzt = jetzt_ms();
        let (rohdaten, _kennung) = antwort::bauen(&Antwortentwurf {
            von_name: Some(&konto.beschriftung),
            von_adresse: &konto.adresse,
            an: &bezug.von_adresse,
            betreff: &bezug.betreff,
            text: &text,
            bezug: bezug.message_id.as_deref(),
            references: &[],
            gesendet_am: jetzt,
        });

        let backend = self.backend(&konto_id)?;
        backend
            .senden(&rohdaten, &konto.adresse, &[bezug.von_adresse.clone()])
            .await?;

        // Ab hier ist die Mail weg. Was jetzt noch schiefgeht, darf den
        // Versand nicht nachträglich zum Fehler machen.
        match self.gesendet_ablegen(&backend, &konto_id, &rohdaten, jetzt).await {
            Ok(ordner) => tracing::debug!(%ordner, "Kopie abgelegt"),
            Err(fehler) => {
                tracing::warn!(%fehler, "Kopie in Gesendet fehlgeschlagen");
                self.hoerer.hoere(crate::ereignis::Ereignis::Stoerung {
                    text: format!(
                        "Die Antwort ist unterwegs, aber die Kopie in „Gesendet“ \
                         konnte nicht abgelegt werden: {fehler}"
                    ),
                });
            }
        }

        Ok(())
    }

    pub async fn nachricht(&self, _nachricht_id: String) -> Ergebnis<NachrichtAnsicht> {
        Err(Fehler::offen("Einzelne Nachricht — die Oberfläche holt das Gespräch"))
    }

    /// Setzt ein ganzes Gespräch auf gelesen oder ungelesen.
    pub async fn gelesen_setzen(
        &self,
        ordner_id: String,
        thread_id: String,
        gelesen: bool,
    ) -> Ergebnis<()> {
        self.flagge_setzen(&ordner_id, &thread_id, "gelesen", gelesen).await
    }

    pub async fn markieren(
        &self,
        ordner_id: String,
        thread_id: String,
        markiert: bool,
    ) -> Ergebnis<()> {
        self.flagge_setzen(&ordner_id, &thread_id, "markiert", markiert).await
    }

    /// Wie viele lokale Änderungen noch auf den Server warten.
    ///
    /// Die Oberfläche kann das anzeigen — „3 Änderungen noch nicht
    /// übertragen" ist ehrlicher als ein Haken, der so tut, als wäre alles
    /// erledigt.
    pub async fn offene_auftraege(&self, konto_id: String) -> Ergebnis<u32> {
        let offen = self
            .speicher()?
            .auftraege(&konto_id, 1000)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
        Ok(offen.len() as u32)
    }

    /// Arbeitet ab, was lokal passiert ist und noch nicht beim Server ist.
    ///
    /// Scheitert ein Auftrag, wird er gezählt und bleibt liegen — der
    /// nächste Abgleich nimmt ihn wieder mit. Nach `VERSUCHE_MAX`
    /// Fehlversuchen bleibt er endgültig liegen, statt jeden weiteren
    /// Abgleich zu blockieren.
    pub async fn warteschlange_abarbeiten(&self, konto_id: String) -> Ergebnis<u32> {
        let auftraege = self
            .speicher()?
            .auftraege(&konto_id, 100)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

        if auftraege.is_empty() {
            return Ok(0);
        }

        let backend = self.backend(&konto_id)?;
        let mut erledigt = 0;

        for auftrag in auftraege {
            match self.auftrag_ausfuehren(&backend, &auftrag).await {
                Ok(()) => {
                    self.speicher()?
                        .auftrag_erledigt(auftrag.id)
                        .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
                    erledigt += 1;
                }
                Err(fehler) => {
                    tracing::warn!(auftrag = auftrag.id, %fehler, "Auftrag gescheitert");
                    self.speicher()?
                        .auftrag_gescheitert(auftrag.id, &fehler.to_string())
                        .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
                }
            }
        }

        Ok(erledigt)
    }

    pub async fn archivieren(&self, _thread_id: String) -> Ergebnis<()> {
        Err(Fehler::offen("Verschieben — Aufgabe #9863"))
    }

    pub async fn loeschen(&self, _thread_id: String) -> Ergebnis<()> {
        Err(Fehler::offen("Verschieben — Aufgabe #9863"))
    }

    /// Bis `bis` (Unix-Millisekunden) aus dem Eingang nehmen.
    pub async fn schlummern(&self, _thread_id: String, _bis: i64) -> Ergebnis<()> {
        Err(Fehler::offen("Schlummern — Aufgabe #9864"))
    }

    pub async fn suchen(&self, _abfrage: String, _anzahl: u32) -> Ergebnis<ThreadSeite> {
        Err(Fehler::offen("Suche über FTS5 — Aufgabe #9864"))
    }

    /// Holt die neuesten Kopfzeilen eines Ordners und schreibt sie.
    ///
    /// Noch ohne CONDSTORE und ohne Warteschlange — das ist Aufgabe #9864.
    /// Was hier schon richtig ist: Es wird einmal geholt, einmal zugeordnet,
    /// einmal geschrieben, und die Oberfläche erfährt es über ein Ereignis.
    pub async fn abgleichen(&self, ordner_id: String) -> Ergebnis<()> {
        let (konto_id, pfad) = zerlege(&ordner_id)?;

        self.hoerer.hoere(crate::ereignis::Ereignis::AbgleichLaeuft {
            konto_id: konto_id.clone(),
            ordner_id: ordner_id.clone(),
        });

        // ZUERST die Warteschlange, DANN lesen. Andersherum überschreibt
        // der Stand vom Server die lokale Änderung, die noch gar nicht dort
        // angekommen ist — und die als gelesen markierte Mail ist wieder
        // ungelesen.
        if let Err(fehler) = self.warteschlange_abarbeiten(konto_id.clone()).await {
            tracing::warn!(%fehler, "Warteschlange vor dem Abgleich nicht leer");
        }

        let backend = self.backend(&konto_id)?;
        let seite = backend.kopfzeilen(&pfad, None, 200).await?;
        let anzahl = seite.kopfzeilen.len() as u32;

        {
            let mut speicher = self.speicher()?;
            speicher
                .ordner_sichern(&ordner_id, &konto_id, &pfad, '/')
                .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

            // Die UIDVALIDITY ZUERST prüfen, vor dem Schreiben.
            //
            // Ändert sie sich, zeigt jede gespeicherte UID auf eine andere
            // Nachricht. Wer dann einfach weiterschreibt, behält alte
            // Betreffs unter neuen Kennungen — die Liste zeigt Nachrichten,
            // die es so nie gab, und die echten fehlen. Genau das ist am
            // 12.11.2026 passiert, nachdem der Testserver neu gestartet
            // wurde.
            if let Some(vom_server) = seite.uidvalidity {
                let gespeichert = speicher
                    .uidvalidity(&ordner_id)
                    .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

                match gespeichert {
                    Some(alt) if alt != vom_server => {
                        let geloescht = speicher
                            .ordner_zuruecksetzen(&ordner_id, vom_server)
                            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
                        tracing::warn!(
                            %ordner_id, alt, neu = vom_server, geloescht,
                            "UIDVALIDITY hat sich geändert — Ordner neu gelesen"
                        );
                        self.hoerer.hoere(crate::ereignis::Ereignis::Stoerung {
                            text: format!(
                                "Der Ordner {pfad} wurde auf dem Server neu aufgebaut und \
                                 deshalb vollständig neu geladen."
                            ),
                        });
                    }
                    Some(_) => {}
                    None => speicher
                        .uidvalidity_merken(&ordner_id, vom_server)
                        .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?,
                }
            }
            abgleich::einordnen(
                &mut speicher,
                &konto_id,
                &ordner_id,
                seite.kopfzeilen,
                jetzt_ms(),
            )
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
        }

        self.hoerer.hoere(crate::ereignis::Ereignis::AbgleichFertig {
            konto_id,
            ordner_id,
            neue: anzahl,
        });

        Ok(())
    }
}

impl Kern {
    /// Lokal sofort, beim Server irgendwann.
    ///
    /// Die Reihenfolge ist der Kern der Sache: **erst lokal, dann in die
    /// Warteschlange, dann versuchen.** Die Oberfläche zeigt die Änderung
    /// sofort, und ob der Server gerade erreichbar ist, spielt keine Rolle.
    /// Umgekehrt — erst zum Server, dann lokal — hängt jede Änderung an der
    /// Netzverbindung, und im Zug tut sich nichts.
    async fn flagge_setzen(
        &self,
        ordner_id: &str,
        thread_id: &str,
        flagge: &str,
        gesetzt: bool,
    ) -> Ergebnis<()> {
        let (konto_id, _pfad) = zerlege(ordner_id)?;

        let nachrichten = {
            let speicher = self.speicher()?;
            let nachrichten = speicher
                .nachrichten_im_thread(ordner_id, thread_id)
                .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;

            for nachricht in &nachrichten {
                match flagge {
                    "gelesen" => speicher
                        .gelesen_merken(&nachricht.id, gesetzt)
                        .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?,
                    "markiert" => speicher
                        .markiert_merken(&nachricht.id, gesetzt)
                        .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?,
                    andere => {
                        return Err(Fehler::NichtGefunden {
                            was: format!("Flagge {andere}"),
                        });
                    }
                }

                let auftrag = mail_sync::AusstehendeOperation::FlaggeSetzen {
                    nachricht_id: nachricht.id.clone(),
                    flagge: flagge.to_owned(),
                    gesetzt,
                };
                let nutzlast = serde_json::to_string(&auftrag).map_err(|fehler| {
                    Fehler::Speicher { grund: format!("Auftrag nicht schreibbar: {fehler}") }
                })?;
                speicher
                    .auftrag_anlegen(&konto_id, "flagge", &nutzlast)
                    .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?;
            }

            nachrichten
        };

        if nachrichten.is_empty() {
            return Err(Fehler::NichtGefunden {
                was: format!("Gespräch {thread_id}"),
            });
        }

        // Der Versuch, es gleich loszuwerden. Scheitert er, ist das KEIN
        // Fehler für den Aufrufer: Lokal stimmt alles, und der nächste
        // Abgleich nimmt den Auftrag mit.
        if let Err(fehler) = self.warteschlange_abarbeiten(konto_id).await {
            tracing::debug!(%fehler, "Warteschlange noch nicht leer");
        }

        Ok(())
    }

    async fn auftrag_ausfuehren(
        &self,
        backend: &Box<dyn Kontobackend>,
        auftrag: &mail_store::Auftrag,
    ) -> Ergebnis<()> {
        let operation: mail_sync::AusstehendeOperation = serde_json::from_str(
            &auftrag.nutzlast_json,
        )
        .map_err(|fehler| Fehler::Speicher {
            grund: format!("Auftrag unlesbar: {fehler}"),
        })?;

        match operation {
            mail_sync::AusstehendeOperation::FlaggeSetzen {
                nachricht_id,
                flagge,
                gesetzt,
            } => {
                let (ordner_id, kennung) = kennung_zerlegen(&nachricht_id)?;
                let (_konto, pfad) = zerlege(&ordner_id)?;
                let flagge = match flagge.as_str() {
                    "gelesen" => mail_proto::Flagge::Gelesen,
                    "markiert" => mail_proto::Flagge::Markiert,
                    andere => mail_proto::Flagge::Eigen(andere.to_owned()),
                };

                let (setzen, loeschen): (&[mail_proto::Flagge], &[mail_proto::Flagge]) =
                    if gesetzt {
                        (std::slice::from_ref(&flagge), &[])
                    } else {
                        (&[], std::slice::from_ref(&flagge))
                    };

                backend.flaggen_setzen(&pfad, &kennung, setzen, loeschen).await?;
                Ok(())
            }
            andere => Err(Fehler::NochNichtGebaut {
                was: format!("Auftragsart {andere:?}"),
            }),
        }
    }

    /// Legt die Kopie in den Gesendet-Ordner.
    ///
    /// Der Ordner wird über seine ROLLE gesucht, nicht über den Namen: Er
    /// heisst je nach Anbieter und Spracheinstellung „Sent", „Gesendet",
    /// „Sent Items" oder „[Gmail]/Gesendet".
    async fn gesendet_ablegen(
        &self,
        backend: &Box<dyn Kontobackend>,
        konto_id: &str,
        rohdaten: &[u8],
        jetzt: i64,
    ) -> Ergebnis<String> {
        let ordner = backend.ordner().await?;
        let ziel = gesendet_finden(&ordner).ok_or_else(|| Fehler::NichtGefunden {
            was: format!("Gesendet-Ordner in Konto {konto_id}"),
        })?;

        // Mit dem Zeitpunkt des Sendens, nicht dem des Ablegens — und als
        // gelesen, denn man hat sie ja selbst geschrieben.
        backend
            .anhaengen(&ziel, rohdaten, jetzt, &[mail_proto::Flagge::Gelesen])
            .await?;

        Ok(ziel)
    }
}

/// Was NICHT über die Grenze geht. Eigener Block ohne `uniffi::export`:
/// Das Attribut nimmt jede Methode seines Blocks mit, unabhängig von der
/// Sichtbarkeit, und scheitert dann an Rückgabetypen, die es in Swift und
/// Kotlin nicht gibt.
impl Kern {
    /// Die Verbindungsdaten eines Kontos, Geheimnis inklusive. Nur für den
    /// Kern selbst — über die FFI-Grenze geht das nicht hinaus.
    ///
    /// Noch ruft es nur der Test; der erste Aufrufer im Betrieb ist die
    /// IMAP-Anbindung (Aufgabe #9863).
    #[allow(dead_code)]
    pub(crate) fn zugang(&self, konto_id: &str) -> Ergebnis<(Verbindungsdaten, String)> {
        let zeile = self
            .speicher()?
            .konto(konto_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?
            .ok_or_else(|| Fehler::NichtGefunden { was: format!("Konto {konto_id}") })?;

        let rohdaten = zeile.zugang_json.ok_or_else(|| Fehler::NichtGefunden {
            was: format!("Verbindungsdaten für {konto_id}"),
        })?;
        let verbindung: Verbindungsdaten =
            serde_json::from_str(&rohdaten).map_err(|fehler| Fehler::Speicher {
                grund: format!("Verbindungsdaten unlesbar: {fehler}"),
            })?;

        let kennung = zeile.tresor_kennung.unwrap_or_else(|| tresor_kennung(konto_id));
        let geheimnis = self.tresor.lies(kennung)?.ok_or(Fehler::AnmeldungAbgelehnt {
            grund: "kein Geheimnis im Tresor — Konto neu anmelden".into(),
        })?;

        Ok((verbindung, geheimnis))
    }

    /// Baut das Protokoll-Backend für ein Konto. Jeder Aufruf baut ein
    /// neues — die Wiederverwendung der Verbindung steckt im Backend selbst,
    /// und eine Verbindungsverwaltung über Kontogrenzen hinweg gehört in den
    /// Abgleich (#9864), nicht hierher.
    fn backend(&self, konto_id: &str) -> Ergebnis<Box<dyn Kontobackend>> {
        let (verbindung, geheimnis) = self.zugang(konto_id)?;
        let protokoll = self
            .speicher()?
            .konto(konto_id)
            .map_err(|fehler| Fehler::Speicher { grund: fehler.to_string() })?
            .map(|zeile| Protokoll::aus_text(&zeile.protokoll))
            .unwrap_or(Protokoll::Imap);

        let verschluesselung = match verbindung.verschluesselung {
            crate::konto::Verschluesselung::Tls => mail_proto::Verschluesselung::Tls,
            crate::konto::Verschluesselung::Starttls => mail_proto::Verschluesselung::Starttls,
        };

        let zugang = Zugang {
            host: verbindung.host.clone(),
            port: verbindung.port,
            benutzer: verbindung.benutzer.clone(),
            geheimnis,
            verschluesselung,
            senden: Some(mail_proto::Sendeziel {
                host: verbindung
                    .smtp_host
                    .clone()
                    .unwrap_or_else(|| verbindung.host.clone()),
                // 465 ist implizites TLS. 587 wäre STARTTLS — das kann der
                // Client noch nicht, und ein stiller Rückfall darauf wäre
                // eine Klartextverbindung.
                port: verbindung.smtp_port.unwrap_or(465),
                verschluesselung: mail_proto::Verschluesselung::Tls,
            }),
        };

        Ok(match protokoll {
            Protokoll::Imap => {
                // Der ungeprüfte Weg existiert nur mit `test-support` und
                // steht deshalb hier als cfg, nicht als Feld am Konto: Ein
                // Feld landete irgendwann als Haken in der Oberfläche.
                #[cfg(feature = "test-support")]
                {
                    Box::new(mail_proto::imap::ImapBackend::neu_ungeprueft(zugang))
                }
                #[cfg(not(feature = "test-support"))]
                {
                    Box::new(mail_proto::imap::ImapBackend::neu(zugang))
                }
            }
            Protokoll::Jmap => Box::new(mail_proto::jmap::JmapBackend::neu(
                // Ohne eigene Sitzungsadresse der Host selbst — JMAP findet
                // den Rest über `/.well-known/jmap`.
                verbindung
                    .jmap_sitzung
                    .clone()
                    .unwrap_or_else(|| format!("https://{}", verbindung.host)),
                verbindung.benutzer.clone(),
                zugang.geheimnis,
            )),
        })
    }

    fn speicher(&self) -> Ergebnis<MutexGuard<'_, Speicher>> {
        // Eine vergiftete Sperre heisst: ein anderer Thread ist WÄHREND eines
        // Schreibvorgangs gestorben. Weiterarbeiten wäre Arbeiten auf einem
        // Stand, von dem niemand weiss, wie halb er ist.
        self.speicher.lock().map_err(|_| Fehler::Speicher {
            grund: "Sperre vergiftet — ein voriger Zugriff ist abgebrochen".into(),
        })
    }

}

fn als_ansicht(zeile: KontoZeile) -> KontoAnsicht {
    KontoAnsicht {
        id: zeile.id,
        beschriftung: zeile.beschriftung,
        adresse: zeile.adresse,
        gesundheit: match zeile.gesundheit.as_deref() {
            Some("gut") => Gesundheit::Gut,
            Some(grund) => Gesundheit::Kaputt { grund: grund.to_owned() },
            // Noch nie verbunden — das ist kein Fehler, und die Oberfläche
            // soll dafür kein rotes Zeichen setzen.
            None => Gesundheit::Unbekannt,
        },
        // Kommt aus dem Abgleich, Aufgabe #9864.
        ungelesen: 0,
    }
}

/// `<konto-id>/<pfad>` wieder auseinandernehmen.
///
/// Die Kontokennung ist eine UUID und enthält keinen Schrägstrich; ein
/// Ordnerpfad darf einen enthalten. Deshalb wird beim ERSTEN getrennt und
/// nicht beim letzten.
fn zerlege(ordner_id: &str) -> Ergebnis<(String, String)> {
    ordner_id
        .split_once('/')
        .map(|(konto, pfad)| (konto.to_owned(), pfad.to_owned()))
        .ok_or_else(|| Fehler::NichtGefunden {
            was: format!("Ordnerkennung „{ordner_id}“ ohne Konto"),
        })
}

/// `<konto>/<pfad>/<kennung>` → (`<konto>/<pfad>`, kennung).
///
/// Von RECHTS trennen: Der Ordnerpfad darf Schrägstriche enthalten, die
/// Serverkennung nicht — weder eine IMAP-UID noch eine JMAP-Id.
fn kennung_zerlegen(nachricht_id: &str) -> Ergebnis<(String, String)> {
    nachricht_id
        .rsplit_once('/')
        .map(|(ordner_id, kennung)| (ordner_id.to_owned(), kennung.to_owned()))
        .ok_or_else(|| Fehler::NichtGefunden {
            was: format!("Nachrichtenkennung „{nachricht_id}“"),
        })
}

fn jetzt_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dauer| dauer.as_millis() as i64)
        .unwrap_or(0)
}

/// Sucht den Gesendet-Ordner — erst über die Rolle, dann über den Namen.
///
/// Die Rolle (SPECIAL-USE, RFC 6154) ist die verlässliche Antwort, aber
/// längst nicht jeder Server gibt sie heraus; GreenMail etwa meldet gar
/// keine Merkmale, und einige verbreitete Server auch nicht. Ohne
/// Namens-Rückfall hätte die Kopie dort nie ein Ziel.
///
/// Der Rückfall ist bewusst eine feste Liste und kein „enthält sent": Ein
/// Ordner „Rechnungen gesendet 2024" ist nicht der Postausgang.
fn gesendet_finden(ordner: &[mail_proto::Ordner]) -> Option<String> {
    const NAMEN: &[&str] = &[
        "sent",
        "gesendet",
        "sent items",
        "sent messages",
        "gesendete elemente",
        "gesendete objekte",
        "inbox.sent",
        "inbox.gesendet",
    ];

    if let Some(ueber_rolle) = ordner
        .iter()
        .find(|ordner| ordner.rolle.as_deref() == Some("sent"))
    {
        return Some(ueber_rolle.pfad.clone());
    }

    ordner
        .iter()
        .find(|eintrag| {
            let name = eintrag.pfad.to_lowercase();
            // Auch den letzten Pfadteil prüfen: „INBOX.Gesendet" heisst in
            // der Liste so, der Ordner aber „Gesendet".
            let letzter = name
                .rsplit(|zeichen| zeichen == '/' || zeichen == '.')
                .next()
                .unwrap_or(&name)
                .to_owned();
            NAMEN.contains(&name.as_str()) || NAMEN.contains(&letzter.as_str())
        })
        .map(|eintrag| eintrag.pfad.clone())
}

/// Die Rolle kommt aus SPECIAL-USE, nicht aus dem Namen — mit INBOX als
/// einziger Ausnahme, denn die ist in RFC 3501 namentlich festgelegt.
fn rolle(ordner: &mail_proto::Ordner) -> Ordnerrolle {
    if ordner.pfad.eq_ignore_ascii_case("INBOX") {
        return Ordnerrolle::Eingang;
    }

    match ordner.rolle.as_deref() {
        Some("sent") => Ordnerrolle::Gesendet,
        Some("drafts") => Ordnerrolle::Entwuerfe,
        Some("trash") => Ordnerrolle::Papierkorb,
        Some("junk") => Ordnerrolle::Spam,
        Some("archive") => Ordnerrolle::Archiv,
        _ => Ordnerrolle::Sonstiger,
    }
}

/// Der Protokoll-Begriff der Datenbank ist eine Zeichenkette; hier wird sie
/// wieder zum Typ. Steht hier und nicht in der Oberfläche, damit nicht jede
/// der drei Plattformen ihre eigene Lesart erfindet.
pub fn protokoll_aus_text(text: &str) -> Protokoll {
    Protokoll::aus_text(text)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::ereignis::Ereignis;
    use crate::konto::Verschluesselung;

    /// Tresor für Tests. Nicht der echte Schlüsselbund: Ein Test, der in den
    /// Schlüsselbund des Benutzers schreibt, hinterlässt dort Einträge und
    /// scheitert auf jedem Rechner ohne laufenden Dienst.
    #[derive(Default)]
    struct Taschentresor {
        inhalt: StdMutex<HashMap<String, String>>,
    }

    impl Tresor for Taschentresor {
        fn lies(&self, kennung: String) -> Ergebnis<Option<String>> {
            Ok(self.inhalt.lock().unwrap().get(&kennung).cloned())
        }

        fn schreib(&self, kennung: String, geheimnis: String) -> Ergebnis<()> {
            self.inhalt.lock().unwrap().insert(kennung, geheimnis);
            Ok(())
        }

        fn loesche(&self, kennung: String) -> Ergebnis<()> {
            self.inhalt.lock().unwrap().remove(&kennung);
            Ok(())
        }
    }

    /// Ein Tresor, der sich weigert — für den Fall, der im Betrieb wirklich
    /// vorkommt: kein Schlüsselbund-Dienst, oder der Benutzer bricht die
    /// Freigabe ab.
    struct SperrigerTresor;

    impl Tresor for SperrigerTresor {
        fn lies(&self, _kennung: String) -> Ergebnis<Option<String>> {
            Ok(None)
        }

        fn schreib(&self, _kennung: String, _geheimnis: String) -> Ergebnis<()> {
            Err(Fehler::Speicher { grund: "kein Tresor".into() })
        }

        fn loesche(&self, _kennung: String) -> Ergebnis<()> {
            Ok(())
        }
    }

    struct Taub;

    impl EreignisHoerer for Taub {
        fn hoere(&self, _ereignis: Ereignis) {}
    }

    fn entwurf() -> Kontoentwurf {
        Kontoentwurf {
            beschriftung: "Arbeit".into(),
            adresse: "post@beispiel.de".into(),
            protokoll: Protokoll::Imap,
            verbindung: Verbindungsdaten {
                host: "mail.beispiel.de".into(),
                port: 993,
                benutzer: "post@beispiel.de".into(),
                verschluesselung: Verschluesselung::Tls,
                jmap_sitzung: None,
                smtp_host: Some("localhost".into()),
                smtp_port: Some(3465),
            },
            geheimnis: "Streng-geheim-42".into(),
        }
    }

    fn kern(tresor: Arc<dyn Tresor>) -> (Arc<Kern>, tempfile::TempDir) {
        let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
        let kern = Kern::neu(
            Einrichtung {
                datenverzeichnis: verzeichnis.path().to_string_lossy().into_owned(),
            },
            tresor,
            Arc::new(Taub),
        )
        .expect("Kern");
        (kern, verzeichnis)
    }

    #[tokio::test]
    async fn anlegen_auflisten_entfernen() {
        let tresor = Arc::new(Taschentresor::default());
        let (kern, _verzeichnis) = kern(tresor.clone());

        assert!(kern.konten().await.expect("leer").is_empty());

        let id = kern.konto_anlegen(entwurf()).await.expect("anlegen");

        let konten = kern.konten().await.expect("auflisten");
        assert_eq!(konten.len(), 1);
        assert_eq!(konten[0].adresse, "post@beispiel.de");
        // Noch nie verbunden ist kein Fehler.
        assert_eq!(konten[0].gesundheit, Gesundheit::Unbekannt);

        // Beide Hälften finden wieder zusammen.
        let (verbindung, geheimnis) = kern.zugang(&id).expect("Zugang");
        assert_eq!(verbindung.host, "mail.beispiel.de");
        assert_eq!(verbindung.port, 993);
        assert_eq!(geheimnis, "Streng-geheim-42");

        kern.konto_entfernen(id.clone()).await.expect("entfernen");
        assert!(kern.konten().await.expect("leer").is_empty());
        assert!(
            tresor.lies(tresor_kennung(&id)).expect("lesen").is_none(),
            "Tresoreintrag blieb liegen"
        );
    }

    #[tokio::test]
    async fn das_geheimnis_steht_nicht_in_der_datenbankdatei() {
        let (kern, verzeichnis) = kern(Arc::new(Taschentresor::default()));
        kern.konto_anlegen(entwurf()).await.expect("anlegen");

        // WAL abschliessen, sonst steckt der Inhalt noch im Begleitjournal
        // und der Test prüft eine halbe Datei.
        drop(kern);

        let mut gefunden = Vec::new();
        for eintrag in std::fs::read_dir(verzeichnis.path()).expect("lesen") {
            let pfad = eintrag.expect("Eintrag").path();
            let rohdaten = std::fs::read(&pfad).unwrap_or_default();
            if rohdaten
                .windows(b"Streng-geheim-42".len())
                .any(|fenster| fenster == b"Streng-geheim-42")
            {
                gefunden.push(pfad);
            }
        }

        assert!(
            gefunden.is_empty(),
            "Das Passwort steht im Klartext auf der Platte: {gefunden:?}"
        );
    }

    #[tokio::test]
    async fn ohne_tresor_entsteht_kein_halbes_konto() {
        let (kern, _verzeichnis) = kern(Arc::new(SperrigerTresor));

        let fehler = kern.konto_anlegen(entwurf()).await;
        assert!(fehler.is_err(), "Anlegen muss scheitern");

        assert!(
            kern.konten().await.expect("auflisten").is_empty(),
            "Es bleibt ein Konto zurück, dessen Geheimnis nirgends liegt"
        );
    }

    #[tokio::test]
    async fn ein_konto_ohne_adresse_wird_abgelehnt() {
        let (kern, _verzeichnis) = kern(Arc::new(Taschentresor::default()));

        let mut ohne = entwurf();
        ohne.adresse = "   ".into();
        assert!(kern.konto_anlegen(ohne).await.is_err());
    }

    #[tokio::test]
    async fn ohne_beschriftung_traegt_das_konto_seine_adresse() {
        let (kern, _verzeichnis) = kern(Arc::new(Taschentresor::default()));

        let mut ohne = entwurf();
        ohne.beschriftung = String::new();
        kern.konto_anlegen(ohne).await.expect("anlegen");

        let konten = kern.konten().await.expect("auflisten");
        assert_eq!(konten[0].beschriftung, "post@beispiel.de");
    }
}

#[cfg(test)]
mod gesendet_tests {
    use super::gesendet_finden;

    fn ordner(pfad: &str, rolle: Option<&str>) -> mail_proto::Ordner {
        mail_proto::Ordner {
            pfad: pfad.to_owned(),
            name: pfad.rsplit(['/', '.']).next().unwrap_or(pfad).to_owned(),
            trenner: '.',
            gesamt: 0,
            ungelesen: 0,
            rolle: rolle.map(str::to_owned),
            uidvalidity: None,
            highestmodseq: None,
        }
    }

    #[test]
    fn die_rolle_schlaegt_den_namen() {
        let liste = [
            ordner("Gesendet", None),
            ordner("Archiv/Postausgang", Some("sent")),
        ];
        assert_eq!(gesendet_finden(&liste).as_deref(), Some("Archiv/Postausgang"));
    }

    #[test]
    fn ohne_rolle_hilft_der_name() {
        for name in ["Sent", "Gesendet", "Sent Items", "Gesendete Elemente", "INBOX.Gesendet"] {
            let liste = [ordner("INBOX", None), ordner(name, None)];
            assert_eq!(
                gesendet_finden(&liste).as_deref(),
                Some(name),
                "nicht gefunden: {name}"
            );
        }
    }

    #[test]
    fn ein_aehnlicher_name_zaehlt_nicht() {
        // Sonst landet die Kopie im Ablageordner eines Kunden.
        let liste = [ordner("INBOX", None), ordner("Rechnungen gesendet 2024", None)];
        assert_eq!(gesendet_finden(&liste), None);
    }

    #[test]
    fn ohne_treffer_kein_rateversuch() {
        let liste = [ordner("INBOX", None), ordner("Entwürfe", None)];
        assert_eq!(gesendet_finden(&liste), None);
    }
}
