//! IMAP über `async-imap`.
//!
//! Was hier bewusst NICHT passiert:
//!
//! - **Kein STARTTLS.** Noch nicht gebaut; implizites TLS (993) deckt alle
//!   grossen Anbieter. Ein Klartext-Modus existiert gar nicht — auch nicht
//!   für „nur lokal", weil genau der Schalter später in der Oberfläche
//!   landet und dann jemand anhakt.
//! - **Kein `\Seen` durch Lesen.** Geholt wird mit `EXAMINE` und
//!   `BODY.PEEK[…]`. Ein Client, der beim Vorschauen ungelesen auf gelesen
//!   dreht, verliert Post im Alltag — und das merkt niemand sofort.


use async_imap::Session;
use async_imap::types::NameAttribute;
use async_trait::async_trait;
use futures::StreamExt;
use mail_parser::MessageParser;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard};
use tokio_rustls::TlsConnector;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

use crate::tls::{self, Vertrauen};
use crate::sasl::XOauth2;
use crate::{
    Anmeldeart, Ergebnis, Flagge, Kontobackend, Kopfzeilen, Ordner, Protokollfehler, Seite,
    Verschluesselung, Zugang,
};

type Strom = Compat<tokio_rustls::client::TlsStream<TcpStream>>;
type Sitzung = Session<Strom>;

/// Die Kopfzeilen, die für eine Gesprächsliste gebraucht werden — und nur
/// die. Eine Liste, die `BODY[]` holt, zieht für 50 Zeilen ein paar Megabyte
/// über eine Mobilverbindung.
const KOPFFELDER: &str = "BODY.PEEK[HEADER.FIELDS (SUBJECT FROM TO CC DATE MESSAGE-ID IN-REPLY-TO REFERENCES)]";

pub struct ImapBackend {
    zugang: Zugang,
    vertrauen: Vertrauen,
    /// Eine Verbindung, wiederverwendet. IMAP-Anmeldungen sind teuer, und
    /// manche Anbieter zählen gleichzeitige Verbindungen scharf mit
    /// (Gmail: 15, dann „Too many simultaneous connections").
    sitzung: Mutex<Option<Sitzung>>,
}

impl ImapBackend {
    pub fn neu(zugang: Zugang) -> Self {
        Self { zugang, vertrauen: Vertrauen::Plattform, sitzung: Mutex::new(None) }
    }

    /// Nur für Tests gegen einen lokalen Testserver mit selbst ausgestelltem
    /// Zertifikat.
    #[cfg(feature = "test-support")]
    pub fn neu_ungeprueft(zugang: Zugang) -> Self {
        Self { zugang, vertrauen: Vertrauen::Ungeprueft, sitzung: Mutex::new(None) }
    }

    async fn anmelden(&self) -> Ergebnis<Sitzung> {
        if self.zugang.verschluesselung == Verschluesselung::Starttls {
            return Err(Protokollfehler::NochNichtGebaut("STARTTLS"));
        }

        let tcp = TcpStream::connect((self.zugang.host.as_str(), self.zugang.port))
            .await
            .map_err(|fehler| {
                Protokollfehler::NichtErreichbar(format!(
                    "{}:{} — {fehler}",
                    self.zugang.host, self.zugang.port
                ))
            })?;

        let name = ServerName::try_from(self.zugang.host.clone()).map_err(|_| {
            Protokollfehler::NichtErreichbar(format!("„{}“ ist kein Hostname", self.zugang.host))
        })?;

        let einrichtung = tls::einrichtung(self.vertrauen).map_err(|fehler| {
            Protokollfehler::NichtErreichbar(format!("TLS nicht einrichtbar: {fehler}"))
        })?;

        let tls = TlsConnector::from(einrichtung)
            .connect(name, tcp)
            .await
            .map_err(|fehler| Protokollfehler::NichtErreichbar(format!("TLS: {fehler}")))?;

        let klient = async_imap::Client::new(tls.compat());

        // Der Fehler kommt jeweils als Tupel zurück, mit dem Klienten darin,
        // damit man es erneut versuchen könnte. Den brauchen wir nicht: Ein
        // abgelehntes Passwort wird beim zweiten Versuch nicht richtig, und
        // ein abgelaufenes Token muss erneuert und nicht wiederholt werden.
        let sitzung = match self.zugang.anmeldeart {
            Anmeldeart::Passwort => klient
                .login(&self.zugang.benutzer, &self.zugang.geheimnis)
                .await
                .map_err(|(fehler, _klient)| {
                    Protokollfehler::AnmeldungAbgelehnt(fehler.to_string())
                })?,
            Anmeldeart::XOauth2 => klient
                .authenticate(
                    "XOAUTH2",
                    XOauth2::neu(&self.zugang.benutzer, &self.zugang.geheimnis),
                )
                .await
                .map_err(|(fehler, _klient)| {
                    // Gmail und Outlook antworten bei abgelaufenem Token mit
                    // derselben Ablehnung wie bei falschem — die Oberfläche
                    // muss daraus „neu anmelden" machen, nicht „Passwort
                    // falsch".
                    Protokollfehler::AnmeldungAbgelehnt(format!("XOAUTH2: {fehler}"))
                })?,
        };

        tracing::debug!(host = %self.zugang.host, "IMAP angemeldet");
        Ok(sitzung)
    }

    async fn sitzung(&self) -> Ergebnis<MutexGuard<'_, Option<Sitzung>>> {
        let mut sperre = self.sitzung.lock().await;
        if sperre.is_none() {
            *sperre = Some(self.anmelden().await?);
        }
        Ok(sperre)
    }

    /// Alle UIDs eines Ordners, absteigend.
    ///
    /// Ja, das holt bei 40.000 Nachrichten 40.000 Zahlen. IMAP kann kein
    /// „die neuesten 50" — es gibt kein LIMIT, und Sequenznummern verschieben
    /// sich, sobald jemand etwas löscht. Sobald der Abgleich (#9864) die UIDs
    /// lokal führt, fragt diese Stelle den Server gar nicht mehr; bis dahin
    /// ist eine Zahlenliste das kleinere Übel gegenüber geratenen Fenstern.
    async fn uids_absteigend(
        sitzung: &mut Sitzung,
        pfad: &str,
    ) -> Ergebnis<(Vec<u32>, Option<u32>)> {
        let postfach = sitzung.examine(pfad).await.map_err(uebersetze)?;
        let uidvalidity = postfach.uid_validity;
        let treffer = sitzung.uid_search("ALL").await.map_err(uebersetze)?;
        let mut uids: Vec<u32> = treffer.into_iter().collect();
        uids.sort_unstable_by(|links, rechts| rechts.cmp(links));
        Ok((uids, uidvalidity))
    }
}

#[async_trait]
impl Kontobackend for ImapBackend {
    async fn ordner(&self) -> Ergebnis<Vec<Ordner>> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        let mut strom = sitzung.list(Some(""), Some("*")).await.map_err(uebersetze)?;
        let mut ordner = Vec::new();
        while let Some(eintrag) = strom.next().await {
            let eintrag = eintrag.map_err(uebersetze)?;

            // Ein Ordner, in dem nichts liegen kann, ist für eine
            // Ordnerliste trotzdem interessant — er trägt Kinder. Einer, den
            // man nicht auswählen kann, darf aber nicht anklickbar sein.
            let auswaehlbar = !eintrag
                .attributes()
                .iter()
                .any(|merkmal| matches!(merkmal, NameAttribute::NoSelect));
            if !auswaehlbar {
                continue;
            }

            let voller_pfad = eintrag.name().to_owned();
            let trenner = eintrag.delimiter().and_then(|d| d.chars().next()).unwrap_or('/');
            ordner.push(Ordner {
                name: voller_pfad
                    .rsplit(trenner)
                    .next()
                    .unwrap_or(&voller_pfad)
                    .to_owned(),
                pfad: voller_pfad,
                trenner,
                gesamt: 0,
                ungelesen: 0,
                rolle: rolle_aus_merkmalen(eintrag.attributes()),
                uidvalidity: None,
                highestmodseq: None,
            });
        }

        Ok(ordner)
    }

    async fn kopfzeilen(&self, pfad: &str, marke: Option<&str>, anzahl: u32) -> Ergebnis<Seite> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        let (uids, uidvalidity) = Self::uids_absteigend(sitzung, pfad).await?;
        // Die Marke ist bei IMAP die zuletzt gelieferte UID.
        let beginn = match marke.and_then(|wert| wert.parse::<u32>().ok()) {
            Some(grenze) => uids.iter().position(|&uid| uid < grenze).unwrap_or(uids.len()),
            None => 0,
        };
        let fenster: Vec<u32> = uids
            .iter()
            .skip(beginn)
            .take(anzahl as usize)
            .copied()
            .collect();

        if fenster.is_empty() {
            return Ok(Seite { kopfzeilen: Vec::new(), weiter_ab: None, uidvalidity });
        }

        let menge = fenster
            .iter()
            .map(|uid| uid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let abfrage = format!("(UID FLAGS INTERNALDATE RFC822.SIZE {KOPFFELDER})");

        let mut strom = sitzung.uid_fetch(menge, abfrage).await.map_err(uebersetze)?;
        let mut kopfzeilen = Vec::new();
        while let Some(antwort) = strom.next().await {
            let antwort = antwort.map_err(uebersetze)?;
            if let Some(zeile) = aus_antwort(&antwort) {
                kopfzeilen.push(zeile);
            }
        }

        // Der Server liefert in beliebiger Reihenfolge; die Oberfläche will
        // neueste zuerst.
        //
        // Sortiert wird nach der REIHENFOLGE IM FENSTER, nicht nach der
        // Kennung: Die ist seit der Umstellung auf JMAP eine Zeichenkette,
        // und dort steht „10" vor „9". Das Fenster ist bereits absteigend
        // nach UID gebaut — es ist die richtige Ordnung, und es kostet
        // nichts, sie zu behalten.
        let reihenfolge: std::collections::HashMap<String, usize> = fenster
            .iter()
            .enumerate()
            .map(|(stelle, uid)| (uid.to_string(), stelle))
            .collect();
        kopfzeilen.sort_by_key(|zeile| {
            reihenfolge
                .get(&zeile.kennung)
                .copied()
                .unwrap_or(usize::MAX)
        });

        let weiter_ab = uids
            .get(beginn + fenster.len())
            .map(|_| fenster.last().copied().expect("Fenster ist nicht leer").to_string());

        Ok(Seite { kopfzeilen, weiter_ab, uidvalidity })
    }

    async fn rohfassung(&self, pfad: &str, kennung: &str) -> Ergebnis<Vec<u8>> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        sitzung.examine(pfad).await.map_err(uebersetze)?;
        let mut strom = sitzung
            .uid_fetch(kennung.to_owned(), "BODY.PEEK[]")
            .await
            .map_err(uebersetze)?;

        while let Some(antwort) = strom.next().await {
            let antwort = antwort.map_err(uebersetze)?;
            if let Some(rohdaten) = antwort.body() {
                return Ok(rohdaten.to_vec());
            }
        }

        Err(Protokollfehler::Unerwartet(format!(
            "Nachricht {kennung} in {pfad} liefert keinen Inhalt"
        )))
    }

    async fn flaggen_setzen(
        &self,
        pfad: &str,
        kennung: &str,
        setzen: &[Flagge],
        loeschen: &[Flagge],
    ) -> Ergebnis<()> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        // SELECT, nicht EXAMINE: Schreiben braucht einen beschreibbaren
        // Ordner.
        sitzung.select(pfad).await.map_err(uebersetze)?;

        for (zeichen, flaggen) in [('+', setzen), ('-', loeschen)] {
            if flaggen.is_empty() {
                continue;
            }
            let liste = flaggen
                .iter()
                .map(als_imap_flagge)
                .collect::<Vec<_>>()
                .join(" ");
            let mut strom = sitzung
                .uid_store(kennung.to_owned(), format!("{zeichen}FLAGS.SILENT ({liste})"))
                .await
                .map_err(uebersetze)?;
            // Der Strom MUSS geleert werden, sonst bleibt die Antwort des
            // Servers im Puffer und landet als Müll vor der nächsten Antwort.
            while let Some(antwort) = strom.next().await {
                antwort.map_err(uebersetze)?;
            }
        }

        Ok(())
    }

    async fn verschieben(&self, von: &str, kennung: &str, nach: &str) -> Ergebnis<()> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        sitzung.select(von).await.map_err(uebersetze)?;
        sitzung.uid_mv(kennung.to_owned(), nach).await.map_err(uebersetze)
    }

    async fn anhaengen(
        &self,
        pfad: &str,
        roh: &[u8],
        empfangen_am: i64,
        flaggen: &[Flagge],
    ) -> Ergebnis<()> {
        let mut sperre = self.sitzung().await?;
        let sitzung = sperre.as_mut().expect("eben gesetzt");

        let flaggenliste = (!flaggen.is_empty()).then(|| {
            format!(
                "({})",
                flaggen.iter().map(als_imap_flagge).collect::<Vec<_>>().join(" ")
            )
        });

        // Das Datum MUSS mit. Ohne es setzt der Server den Zeitpunkt des
        // Einlegens, und eine kopierte Nachricht trägt das Datum des
        // Umzugstags — in einer nach Datum sortierten Liste rutscht sie
        // damit nach oben und wirkt wie neu.
        let datum = crate::zeitpunkt::als_imap_datum(empfangen_am);

        sitzung
            .append(pfad, flaggenliste.as_deref(), Some(&datum), roh)
            .await
            .map_err(uebersetze)
    }

    async fn senden(&self, roh: &[u8], absender: &str, empfaenger: &[String]) -> Ergebnis<()> {
        let Some(ziel) = &self.zugang.senden else {
            return Err(Protokollfehler::Unerwartet(
                "für dieses Konto ist kein Postausgang eingerichtet".to_owned(),
            ));
        };

        // `new` prüft den Hostnamen und gibt deshalb ein Result zurück —
        // der Fehler ist eine Zeichenkette, keine Fehlerart.
        let anmeldung = match self.zugang.anmeldeart {
            Anmeldeart::Passwort => mail_send::Credentials::Plain {
                username: self.zugang.benutzer.clone(),
                secret: self.zugang.geheimnis.clone(),
            },
            Anmeldeart::XOauth2 => mail_send::Credentials::XOauth2 {
                username: self.zugang.benutzer.clone(),
                secret: self.zugang.geheimnis.clone(),
            },
        };

        let bauer = mail_send::SmtpClientBuilder::new(ziel.host.clone(), ziel.port)
            .map_err(|grund| {
                Protokollfehler::NichtErreichbar(format!("Postausgang: {grund}"))
            })?
            .implicit_tls(ziel.verschluesselung == Verschluesselung::Tls)
            .credentials(anmeldung);

        #[cfg(feature = "test-support")]
        let bauer = if self.vertrauen == Vertrauen::Ungeprueft {
            bauer.allow_invalid_certs()
        } else {
            bauer
        };

        let mut klient = bauer.connect().await.map_err(uebersetze_smtp)?;

        klient
            .send(mail_send::smtp::message::Message::new(
                absender.to_owned(),
                empfaenger.to_vec(),
                roh.to_vec(),
            ))
            .await
            .map_err(uebersetze_smtp)?;

        Ok(())
    }
}

/// Die Rolle eines Ordners aus den SPECIAL-USE-Merkmalen (RFC 6154) — nicht
/// aus dem Namen. Ein Ordner „Archiv" kann ein gewöhnlicher sein, und
/// „[Gmail]/All Mail" ist keiner, obwohl er wie einer klingt.
///
/// **Die typisierten Varianten zuerst.** `imap-proto` übersetzt `\Sent`,
/// `\Drafts`, `\Trash`, `\Junk`, `\Archive`, `\All` und `\Flagged` in
/// eigene Aufzählungswerte — sie landen NICHT in `Extension`. Wer nur dort
/// nachsieht, wirft jede Rolle stillschweigend weg und merkt es nicht:
/// Ein Server ohne SPECIAL-USE (GreenMail etwa) verhält sich genauso.
/// Gefunden am 12.11.2026 gegen den echten Stalwart, der alle vier schickt.
fn rolle_aus_merkmalen(merkmale: &[NameAttribute<'_>]) -> Option<String> {
    merkmale.iter().find_map(|merkmal| {
        let rolle = match merkmal {
            NameAttribute::Sent => "sent",
            NameAttribute::Drafts => "drafts",
            NameAttribute::Trash => "trash",
            NameAttribute::Junk => "junk",
            NameAttribute::Archive => "archive",
            NameAttribute::All => "all",
            NameAttribute::Flagged => "flagged",
            // Für Server, die eigene Merkmale schicken. Hier steht der
            // Gegenschrägstrich noch davor.
            NameAttribute::Extension(wert) => {
                let wert = wert.trim_start_matches('\\').to_ascii_lowercase();
                return matches!(
                    wert.as_str(),
                    "sent" | "drafts" | "trash" | "junk" | "archive" | "all" | "flagged"
                )
                .then_some(wert);
            }
            _ => return None,
        };
        Some(rolle.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn die_typisierten_merkmale_werden_erkannt() {
        // Genau der Fall, der still ausfiel: imap-proto liefert `\Sent` als
        // eigene Variante, nicht als Extension.
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::Sent]).as_deref(),
            Some("sent")
        );
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::Trash]).as_deref(),
            Some("trash")
        );
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::Drafts]).as_deref(),
            Some("drafts")
        );
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::Junk]).as_deref(),
            Some("junk")
        );
    }

    #[test]
    fn unbekannte_merkmale_ueber_die_erweiterung() {
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::Extension("\\Archive".into())]).as_deref(),
            Some("archive")
        );
    }

    #[test]
    fn gewoehnliche_merkmale_ergeben_keine_rolle() {
        assert_eq!(
            rolle_aus_merkmalen(&[NameAttribute::NoInferiors, NameAttribute::Unmarked]),
            None
        );
        assert_eq!(rolle_aus_merkmalen(&[]), None);
    }
}

fn als_imap_flagge(flagge: &Flagge) -> String {
    match flagge {
        Flagge::Gelesen => "\\Seen".to_owned(),
        Flagge::Markiert => "\\Flagged".to_owned(),
        Flagge::Beantwortet => "\\Answered".to_owned(),
        Flagge::Entwurf => "\\Draft".to_owned(),
        Flagge::Geloescht => "\\Deleted".to_owned(),
        // $Forwarded ist ein Schlüsselwort, kein Systemflag — ohne
        // Gegenschrägstrich.
        Flagge::Weitergeleitet => "$Forwarded".to_owned(),
        Flagge::Eigen(name) => name.clone(),
    }
}

fn aus_flagge(flagge: &async_imap::types::Flag<'_>) -> Flagge {
    use async_imap::types::Flag;
    match flagge {
        Flag::Seen => Flagge::Gelesen,
        Flag::Flagged => Flagge::Markiert,
        Flag::Answered => Flagge::Beantwortet,
        Flag::Draft => Flagge::Entwurf,
        Flag::Deleted => Flagge::Geloescht,
        Flag::Custom(name) if name.eq_ignore_ascii_case("$forwarded") => Flagge::Weitergeleitet,
        Flag::Custom(name) => Flagge::Eigen(name.to_string()),
        sonstige => Flagge::Eigen(format!("{sonstige:?}")),
    }
}

fn aus_antwort(antwort: &async_imap::types::Fetch) -> Option<Kopfzeilen> {
    let uid = antwort.uid?;
    let kennung = uid.to_string();
    let rohkopf = antwort.header().unwrap_or_default();

    // Geparst wird mit `mail-parser`, nicht mit ENVELOPE: Betreffzeilen
    // kommen als `=?UTF-8?B?…?=` und in Zeichensätzen, die vor 1998 Mode
    // waren. Das einmal richtig zu machen ist eine Bibliothek wert.
    let geparst = MessageParser::default().parse_headers(rohkopf);

    let (betreff, von, gesendet_am, message_id, in_reply_to, references) = match &geparst {
        Some(kopf) => (
            kopf.subject().unwrap_or_default().to_owned(),
            kopf.from()
                .and_then(|liste| liste.first())
                .and_then(|adresse| adresse.address())
                .unwrap_or_default()
                .to_owned(),
            kopf.date().map(|datum| datum.to_timestamp() * 1000),
            kopf.message_id().map(str::to_owned),
            kopf.in_reply_to()
                .and_then(|liste| liste.first().map(|wert| wert.to_string())),
            kopf.references()
                .map(|liste| liste.iter().map(|wert| wert.to_string()).collect())
                .unwrap_or_default(),
        ),
        None => (String::new(), String::new(), None, None, None, Vec::new()),
    };

    Some(Kopfzeilen {
        kennung,
        message_id,
        in_reply_to,
        references,
        betreff,
        von,
        // Ohne Date-Kopfzeile das Empfangsdatum des Servers nehmen. Eine
        // Nachricht ohne beides gibt es nicht; eine ohne Date schon.
        gesendet_am: gesendet_am
            .or_else(|| antwort.internal_date().map(|datum| datum.timestamp_millis()))
            .unwrap_or(0),
        groesse: antwort.size.unwrap_or(0),
        flaggen: antwort.flags().map(|flagge| aus_flagge(&flagge)).collect(),
    })
}

fn uebersetze_smtp(fehler: mail_send::Error) -> Protokollfehler {
    use mail_send::Error;
    match fehler {
        Error::Io(innen) => Protokollfehler::NichtErreichbar(innen.to_string()),
        Error::AuthenticationFailed(innen) => {
            Protokollfehler::AnmeldungAbgelehnt(innen.to_string())
        }
        Error::UnexpectedReply(antwort) => Protokollfehler::Unerwartet(format!("{antwort:?}")),
        sonstige => Protokollfehler::NichtErreichbar(sonstige.to_string()),
    }
}

fn uebersetze(fehler: async_imap::error::Error) -> Protokollfehler {
    use async_imap::error::Error;
    match fehler {
        Error::Io(innen) => Protokollfehler::NichtErreichbar(innen.to_string()),
        Error::ConnectionLost => {
            Protokollfehler::NichtErreichbar("Verbindung abgebrochen".to_owned())
        }
        Error::No(text) | Error::Bad(text) => Protokollfehler::Unerwartet(format!("{text:?}")),
        sonstige => Protokollfehler::Unerwartet(sonstige.to_string()),
    }
}
