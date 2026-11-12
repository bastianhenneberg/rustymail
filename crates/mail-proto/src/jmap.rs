//! JMAP (RFC 8620/8621) — die zweite Erfüllung desselben Vertrags.
//!
//! Von Hand gesprochen statt über `jmap-client`: Dessen Vorgabe zieht
//! `aws-lc-rs` als Krypto-Anbieter, und rustls hätte damit zwei zur Auswahl.
//! Was ein Mail-Client braucht, sind ausserdem vier Methodenaufrufe —
//! `Mailbox/get`, `Email/query`, `Email/get`, `Email/set` — und die sind in
//! JSON überschaubarer als eine fremde API.
//!
//! **Die drei Stellen, an denen JMAP nicht „IMAP mit JSON" ist:**
//!
//! 1. **Keine UIDs.** `Email/id` ist eine Zeichenkette und gilt über Ordner
//!    hinweg. Deshalb heisst die Kennung im Vertrag `kennung: String` und
//!    nicht `uid: u32` — eine für JMAP erfundene Zahl wäre eine Lüge, die
//!    beim zweiten Abgleich auffliegt.
//! 2. **Ordner sind Zugehörigkeiten.** Eine Nachricht kann in mehreren
//!    `mailboxIds` liegen. „Verschieben" heisst hier: die Menge ersetzen.
//! 3. **Der Server kennt Gespräche selbst** (`threadId`). Die eigene
//!    Zusammenfassung über References bleibt trotzdem — für IMAP, und damit
//!    beide Seiten zum selben Ergebnis kommen.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::zeitpunkt;
use crate::{Ergebnis, Flagge, Kontobackend, Kopfzeilen, Ordner, Protokollfehler, Seite};

/// Was aus dem Sitzungsdokument gebraucht wird.
#[derive(Debug, Clone)]
struct Sitzung {
    api_adresse: String,
    download_vorlage: String,
    konto_id: String,
}

pub struct JmapBackend {
    /// Grundadresse des Servers, z. B. `https://mail.beispiel.de`.
    basis: String,
    benutzer: String,
    geheimnis: String,
    klient: reqwest::Client,
    sitzung: tokio::sync::OnceCell<Sitzung>,
}

impl JmapBackend {
    pub fn neu(basis: String, benutzer: String, geheimnis: String) -> Self {
        // Muss VOR dem Bau des HTTP-Klienten passieren: reqwest greift beim
        // ersten Aufruf auf den Anbieter des Prozesses zu.
        crate::tls::anbieter_sicherstellen();
        Self {
            basis: basis.trim_end_matches('/').to_owned(),
            benutzer,
            geheimnis,
            klient: reqwest::Client::new(),
            sitzung: tokio::sync::OnceCell::new(),
        }
    }

    async fn sitzung(&self) -> Ergebnis<&Sitzung> {
        self.sitzung
            .get_or_try_init(|| async {
                let adresse = format!("{}/.well-known/jmap", self.basis);
                let antwort = self
                    .klient
                    .get(&adresse)
                    .basic_auth(&self.benutzer, Some(&self.geheimnis))
                    .send()
                    .await
                    .map_err(|fehler| {
                        Protokollfehler::NichtErreichbar(format!("{adresse}: {fehler}"))
                    })?;

                if antwort.status() == reqwest::StatusCode::UNAUTHORIZED {
                    return Err(Protokollfehler::AnmeldungAbgelehnt(
                        "JMAP-Sitzung abgelehnt".to_owned(),
                    ));
                }

                let dokument: Value = antwort.json().await.map_err(|fehler| {
                    Protokollfehler::Unerwartet(format!("Sitzungsdokument: {fehler}"))
                })?;

                // Das Konto für Mail steht in `primaryAccounts` — NICHT
                // einfach das erste aus `accounts`: Dort stehen auch geteilte
                // Postfächer und Kalender-Konten.
                let konto_id = dokument
                    .pointer("/primaryAccounts/urn:ietf:params:jmap:mail")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Protokollfehler::Unerwartet(
                            "kein Mail-Konto im Sitzungsdokument".to_owned(),
                        )
                    })?
                    .to_owned();

                let api_adresse = self.vervollstaendigen(
                    dokument.get("apiUrl").and_then(Value::as_str).unwrap_or("/jmap/"),
                );
                let download_vorlage = self.vervollstaendigen(
                    dokument.get("downloadUrl").and_then(Value::as_str).unwrap_or_default(),
                );

                Ok(Sitzung { api_adresse, download_vorlage, konto_id })
            })
            .await
    }

    /// Die Adressen im Sitzungsdokument dürfen relativ sein.
    fn vervollstaendigen(&self, adresse: &str) -> String {
        if adresse.starts_with("http://") || adresse.starts_with("https://") {
            adresse.to_owned()
        } else {
            format!("{}{}", self.basis, adresse)
        }
    }

    /// Ein Methodenaufruf, eine Antwort.
    async fn aufrufen(&self, methode: &str, argumente: Value) -> Ergebnis<Value> {
        let sitzung = self.sitzung().await?;

        let rumpf = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [[methode, argumente, "0"]]
        });

        let antwort = self
            .klient
            .post(&sitzung.api_adresse)
            .basic_auth(&self.benutzer, Some(&self.geheimnis))
            .json(&rumpf)
            .send()
            .await
            .map_err(|fehler| Protokollfehler::NichtErreichbar(fehler.to_string()))?;

        if antwort.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Protokollfehler::AnmeldungAbgelehnt(
                "JMAP-Aufruf abgelehnt".to_owned(),
            ));
        }

        let dokument: Value = antwort
            .json()
            .await
            .map_err(|fehler| Protokollfehler::Unerwartet(fehler.to_string()))?;

        let erster = dokument
            .pointer("/methodResponses/0")
            .ok_or_else(|| Protokollfehler::Unerwartet(format!("keine Antwort auf {methode}")))?;

        // Ein Fehler kommt als eigene „Methode" zurück, nicht als
        // HTTP-Status. Wer nur den Status prüft, hält eine Fehlermeldung für
        // ein Ergebnis und arbeitet mit einer leeren Liste weiter.
        let name = erster.get(0).and_then(Value::as_str).unwrap_or_default();
        if name == "error" {
            let art = erster
                .pointer("/1/type")
                .and_then(Value::as_str)
                .unwrap_or("unbekannt");
            return Err(Protokollfehler::Unerwartet(format!("{methode}: {art}")));
        }

        erster
            .get(1)
            .cloned()
            .ok_or_else(|| Protokollfehler::Unerwartet(format!("leere Antwort auf {methode}")))
    }

    async fn konto_id(&self) -> Ergebnis<String> {
        Ok(self.sitzung().await?.konto_id.clone())
    }
}

#[async_trait]
impl Kontobackend for JmapBackend {
    async fn ordner(&self) -> Ergebnis<Vec<Ordner>> {
        let konto = self.konto_id().await?;
        let antwort = self
            .aufrufen(
                "Mailbox/get",
                json!({
                    "accountId": konto,
                    "ids": null,
                    "properties": ["id", "name", "role", "totalEmails", "unreadEmails"]
                }),
            )
            .await?;

        let liste = antwort
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(liste
            .into_iter()
            .filter_map(|eintrag| {
                let id = eintrag.get("id")?.as_str()?.to_owned();
                let name = eintrag
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                Some(Ordner {
                    // Der Pfad ist die Mailbox-Id: Sie bleibt gleich, wenn
                    // jemand den Ordner umbenennt. Der Name ist nur zum
                    // Anzeigen da.
                    pfad: id,
                    name,
                    trenner: '/',
                    gesamt: eintrag.get("totalEmails").and_then(Value::as_u64).unwrap_or(0) as u32,
                    ungelesen: eintrag
                        .get("unreadEmails")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    // JMAP-Rollen heissen wie die SPECIAL-USE-Merkmale,
                    // nur klein — „sent", „drafts", „trash", „junk".
                    rolle: eintrag
                        .get("role")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    uidvalidity: None,
                    highestmodseq: None,
                })
            })
            .collect())
    }

    async fn kopfzeilen(&self, pfad: &str, marke: Option<&str>, anzahl: u32) -> Ergebnis<Seite> {
        let konto = self.konto_id().await?;
        let beginn: u64 = marke.and_then(|wert| wert.parse().ok()).unwrap_or(0);

        let gefunden = self
            .aufrufen(
                "Email/query",
                json!({
                    "accountId": konto,
                    "filter": { "inMailbox": pfad },
                    "sort": [{ "property": "receivedAt", "isAscending": false }],
                    "position": beginn,
                    "limit": anzahl,
                    "calculateTotal": true
                }),
            )
            .await?;

        let ids: Vec<Value> = gefunden
            .get("ids")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if ids.is_empty() {
            return Ok(Seite { kopfzeilen: Vec::new(), weiter_ab: None, uidvalidity: None });
        }

        let gesamt = gefunden.get("total").and_then(Value::as_u64).unwrap_or(0);
        let gelesen = beginn + ids.len() as u64;
        let weiter_ab = (gelesen < gesamt)
            .then(|| gelesen.to_string());

        let nachrichten = self
            .aufrufen(
                "Email/get",
                json!({
                    "accountId": konto,
                    "ids": ids,
                    "properties": [
                        "id", "blobId", "subject", "from", "receivedAt", "sentAt",
                        "size", "keywords", "messageId", "inReplyTo", "references"
                    ]
                }),
            )
            .await?;

        let liste = nachrichten
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(Seite {
            kopfzeilen: liste.iter().filter_map(als_kopfzeilen).collect(),
            weiter_ab,
            uidvalidity: None,
        })
    }

    async fn rohfassung(&self, _pfad: &str, kennung: &str) -> Ergebnis<Vec<u8>> {
        let konto = self.konto_id().await?;
        let nachricht = self
            .aufrufen(
                "Email/get",
                json!({ "accountId": konto, "ids": [kennung], "properties": ["blobId"] }),
            )
            .await?;

        let blob_id = nachricht
            .pointer("/list/0/blobId")
            .and_then(Value::as_str)
            .ok_or_else(|| Protokollfehler::Unerwartet(format!("kein Blob zu {kennung}")))?;

        let sitzung = self.sitzung().await?;
        let adresse = sitzung
            .download_vorlage
            .replace("{accountId}", &konto)
            .replace("{blobId}", blob_id)
            .replace("{name}", "nachricht.eml")
            .replace("{type}", "message/rfc822");

        let antwort = self
            .klient
            .get(&adresse)
            .basic_auth(&self.benutzer, Some(&self.geheimnis))
            .send()
            .await
            .map_err(|fehler| Protokollfehler::NichtErreichbar(fehler.to_string()))?;

        Ok(antwort
            .bytes()
            .await
            .map_err(|fehler| Protokollfehler::Unerwartet(fehler.to_string()))?
            .to_vec())
    }

    async fn flaggen_setzen(
        &self,
        _pfad: &str,
        kennung: &str,
        setzen: &[Flagge],
        loeschen: &[Flagge],
    ) -> Ergebnis<()> {
        let konto = self.konto_id().await?;

        // JMAP setzt einzelne Schlüsselwörter über Pfadangaben:
        // `keywords/$seen: true` statt einer ganzen Menge. Das ist die
        // Form, die nebenläufige Änderungen anderer Geräte nicht wegwirft.
        let mut aenderungen = serde_json::Map::new();
        for flagge in setzen {
            aenderungen.insert(format!("keywords/{}", als_schluesselwort(flagge)), json!(true));
        }
        for flagge in loeschen {
            aenderungen.insert(format!("keywords/{}", als_schluesselwort(flagge)), Value::Null);
        }

        let antwort = self
            .aufrufen(
                "Email/set",
                json!({ "accountId": konto, "update": { kennung: aenderungen } }),
            )
            .await?;

        pruefe_nicht_geaendert(&antwort, kennung)
    }

    async fn verschieben(&self, _von: &str, kennung: &str, nach: &str) -> Ergebnis<()> {
        let konto = self.konto_id().await?;

        // Die Zugehörigkeit wird ERSETZT, nicht verschoben: Eine Nachricht
        // kann in JMAP in mehreren Ordnern liegen, und „eine Zugehörigkeit
        // weg, eine dazu" wäre bei einer Wiederholung nicht dasselbe.
        let antwort = self
            .aufrufen(
                "Email/set",
                json!({
                    "accountId": konto,
                    "update": { kennung: { "mailboxIds": { nach: true } } }
                }),
            )
            .await?;

        pruefe_nicht_geaendert(&antwort, kennung)
    }

    async fn anhaengen(
        &self,
        _pfad: &str,
        _roh: &[u8],
        _empfangen_am: i64,
        _flaggen: &[Flagge],
    ) -> Ergebnis<()> {
        Err(Protokollfehler::NochNichtGebaut("JMAP Email/import"))
    }

    async fn senden(&self, _roh: &[u8], _absender: &str, _empfaenger: &[String]) -> Ergebnis<()> {
        Err(Protokollfehler::NochNichtGebaut("JMAP EmailSubmission/set"))
    }
}

/// `Email/set` meldet Fehlschläge je Kennung in `notUpdated` — mit HTTP 200.
fn pruefe_nicht_geaendert(antwort: &Value, kennung: &str) -> Ergebnis<()> {
    if let Some(fehler) = antwort.pointer(&format!("/notUpdated/{kennung}")) {
        let art = fehler
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unbekannt");
        return Err(Protokollfehler::Unerwartet(format!(
            "{kennung} nicht geändert: {art}"
        )));
    }
    Ok(())
}

fn als_schluesselwort(flagge: &Flagge) -> String {
    match flagge {
        Flagge::Gelesen => "$seen".to_owned(),
        Flagge::Markiert => "$flagged".to_owned(),
        Flagge::Beantwortet => "$answered".to_owned(),
        Flagge::Weitergeleitet => "$forwarded".to_owned(),
        Flagge::Entwurf => "$draft".to_owned(),
        Flagge::Geloescht => "$deleted".to_owned(),
        Flagge::Eigen(name) => name.to_lowercase(),
    }
}

fn aus_schluesselwort(wort: &str) -> Flagge {
    match wort {
        "$seen" => Flagge::Gelesen,
        "$flagged" => Flagge::Markiert,
        "$answered" => Flagge::Beantwortet,
        "$forwarded" => Flagge::Weitergeleitet,
        "$draft" => Flagge::Entwurf,
        "$deleted" => Flagge::Geloescht,
        sonstiges => Flagge::Eigen(sonstiges.to_owned()),
    }
}

fn als_kopfzeilen(eintrag: &Value) -> Option<Kopfzeilen> {
    let kennung = eintrag.get("id")?.as_str()?.to_owned();

    let zeitpunkt_text = eintrag
        .get("sentAt")
        .and_then(Value::as_str)
        .or_else(|| eintrag.get("receivedAt").and_then(Value::as_str))
        .unwrap_or_default();

    let einzelne_kennung = |feld: &str| -> Option<String> {
        eintrag
            .get(feld)
            .and_then(Value::as_array)
            .and_then(|liste| liste.first())
            .and_then(Value::as_str)
            .map(str::to_owned)
    };

    Some(Kopfzeilen {
        kennung,
        message_id: einzelne_kennung("messageId"),
        in_reply_to: einzelne_kennung("inReplyTo"),
        references: eintrag
            .get("references")
            .and_then(Value::as_array)
            .map(|liste| {
                liste
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        betreff: eintrag
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        von: eintrag
            .pointer("/from/0/email")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        gesendet_am: zeitpunkt::aus_rfc3339(zeitpunkt_text).unwrap_or(0),
        groesse: eintrag.get("size").and_then(Value::as_u64).unwrap_or(0) as u32,
        flaggen: eintrag
            .get("keywords")
            .and_then(Value::as_object)
            .map(|wörter| wörter.keys().map(|wort| aus_schluesselwort(wort)).collect())
            .unwrap_or_default(),
    })
}
