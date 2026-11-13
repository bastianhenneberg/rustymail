//! Der vollständige Rundlauf gegen einen echten Mailserver.
//!
//! Konto anlegen → Nachricht ablegen → abgleichen → Gespräch öffnen →
//! antworten → die Antwort kommt an und die Kopie liegt in „Gesendet".
//!
//! Gegen GreenMail läuft dasselbe in `konto_gegen_greenmail.rs`. Dieser Test
//! prüft, was ein Testserver nicht zeigen kann: ein echtes Zertifikat gegen
//! den Vertrauensspeicher des Systems, echte SPECIAL-USE-Merkmale (daran ist
//! die Rollenerkennung am 12.11.2026 aufgefallen), und eine
//! SMTP-Einlieferung über 465, die wirklich zustellt.
//!
//! Er braucht ein **eigenes Postfach** — niemals ein benutztes: Er
//! verschickt, verschiebt und setzt Flaggen.
//!
//! ```sh
//! RUSTYMAIL_TEST_HOST=mail.example.de \
//! RUSTYMAIL_TEST_USER=rustymail-test@example.de \
//! RUSTYMAIL_TEST_PASS=… \
//!   cargo test -p mail-core --test rundlauf_gegen_stalwart -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mail_core::{
    Anmeldeart, Einrichtung, Ereignis, EreignisHoerer, Ergebnis, Kern, Kontoentwurf, Protokoll,
    Tresor, Verbindungsdaten, Verschluesselung,
};

#[derive(Default)]
struct Taschentresor {
    inhalt: Mutex<HashMap<String, String>>,
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

struct Taub;
impl EreignisHoerer for Taub {
    fn hoere(&self, _ereignis: Ereignis) {}
}

fn umgebung() -> (String, String, String) {
    (
        std::env::var("RUSTYMAIL_TEST_HOST").expect("RUSTYMAIL_TEST_HOST setzen"),
        std::env::var("RUSTYMAIL_TEST_USER").expect("RUSTYMAIL_TEST_USER setzen"),
        std::env::var("RUSTYMAIL_TEST_PASS").expect("RUSTYMAIL_TEST_PASS setzen"),
    )
}

/// Legt eine Nachricht per IMAP APPEND im Eingang ab.
///
/// Der Test versorgt sich selbst. Ein Test, der vorhandene Post voraussetzt,
/// ist beim zweiten Lauf rot — oder schlimmer: Er verbraucht sie.
async fn nachricht_ablegen(kennzeichen: &str) -> String {
    use mail_proto::Kontobackend;

    let (host, benutzer, geheimnis) = umgebung();
    let backend = mail_proto::imap::ImapBackend::neu(mail_proto::Zugang {
        host,
        port: 993,
        benutzer: benutzer.clone(),
        geheimnis,
        verschluesselung: mail_proto::Verschluesselung::Tls,
        anmeldeart: mail_proto::Anmeldeart::Passwort,
        senden: None,
    });

    let betreff = format!("Rundlauf {kennzeichen}");
    // Absender ist das Testpostfach SELBST. Sonst geht die Antwort an eine
    // fremde Adresse hinaus — der erste Lauf hat genau das getan, und eine
    // Testnachricht, die den Server verlässt, ist keine Testnachricht mehr.
    let rohdaten = format!(
        "From: {benutzer}\r\n\
         To: {benutzer}\r\n\
         Subject: {betreff}\r\n\
         Message-ID: <rundlauf-{kennzeichen}@beispiel.de>\r\n\
         Date: Thu, 13 Nov 2026 09:00:00 +0000\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Guten Tag,\r\n\
         \r\n\
         können Sie das bitte prüfen?\r\n\
         \r\n\
         Viele Grüße\r\n"
    );

    backend
        .anhaengen("INBOX", rohdaten.as_bytes(), 1_763_020_800_000, &[])
        .await
        .expect("APPEND gegen den echten Server");

    betreff
}

#[tokio::test]
#[ignore = "braucht ein eigenes Postfach auf einem echten Server"]
async fn der_ganze_weg_von_der_nachricht_bis_zur_antwort() {
    let (host, benutzer, geheimnis) = umgebung();
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");

    let kern = Kern::neu(
        Einrichtung {
            datenverzeichnis: verzeichnis.path().to_string_lossy().into_owned(),
        },
        Arc::new(Taschentresor::default()),
        Arc::new(Taub),
    )
    .expect("Kern");

    let konto = kern
        .konto_anlegen(Kontoentwurf {
            beschriftung: "Rundlauf".into(),
            adresse: benutzer.clone(),
            protokoll: Protokoll::Imap,
            verbindung: Verbindungsdaten {
                host: host.clone(),
                port: 993,
                benutzer: benutzer.clone(),
                verschluesselung: Verschluesselung::Tls,
                anmeldeart: Anmeldeart::Passwort,
                jmap_sitzung: None,
                // Einlieferung über 465 mit direktem TLS. 587 lauscht auf
                // dieser Maschine nicht, und 25 bietet kein AUTH.
                smtp_host: Some(host),
                smtp_port: Some(465),
            },
            geheimnis,
        })
        .await
        .expect("Konto anlegen");

    // 1. Die Ordner kommen MIT Rollen — das kann GreenMail nicht zeigen.
    let ordner = kern.ordner(konto.clone()).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.rolle == mail_core::Ordnerrolle::Eingang)
        .expect("Eingang");
    let gesendet = ordner
        .iter()
        .find(|o| o.rolle == mail_core::Ordnerrolle::Gesendet)
        .expect("Gesendet — die Rolle kommt vom Server, nicht aus dem Namen");
    println!("Ordner: {} · Gesendet heisst „{}\"", ordner.len(), gesendet.beschriftung);

    kern.abgleichen(gesendet.id.clone()).await.expect("Gesendet abgleichen");
    let kopien_vorher: u32 = kern
        .threads(gesendet.id.clone(), None, 200)
        .await
        .expect("Liste")
        .zeilen
        .iter()
        .map(|zeile| zeile.anzahl)
        .sum();

    // 2. Eigene Nachricht ablegen und holen.
    //
    // Die Kennung ist je Lauf eine andere. Mit einer festen bliebe die
    // Nachricht des vorigen Laufs liegen, und beim zweiten Mal stünden zwei
    // gleiche Message-Ids im selben Gespräch — der Test wäre rot, ohne dass
    // am Code etwas falsch wäre.
    let kennzeichen = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Zeit")
        .as_secs()
        .to_string();
    let betreff = nachricht_ablegen(&kennzeichen).await;
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 100).await.expect("Liste");
    let strang = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff == betreff)
        .unwrap_or_else(|| panic!("„{betreff}\" nicht im Eingang"));

    // 3. Öffnen — der Textkörper muss seine Absätze behalten.
    let nachrichten = kern
        .gespraech(eingang.id.clone(), strang.thread_id.clone())
        .await
        .expect("Gespräch");
    assert_eq!(nachrichten.len(), 1);
    let text = nachrichten[0].koerper_text();
    assert!(text.contains("können Sie das bitte prüfen"), "Text fehlt: {text:?}");
    assert!(text.contains('\n'), "die Absätze sind verloren: {text:?}");

    // 4. Antworten — echte SMTP-Einlieferung, die wirklich zustellt.
    kern.antworten(
        eingang.id.clone(),
        strang.thread_id.clone(),
        "Ist geprüft, danke.".to_owned(),
    )
    .await
    .expect("antworten");

    // 5. Die Antwort kommt zurück — an dieselbe Adresse — und hängt am
    //    Gespräch. Das beweist In-Reply-To und References.
    let mut angekommen = false;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");
        let danach = kern.threads(eingang.id.clone(), None, 100).await.expect("Liste");
        if danach
            .zeilen
            .iter()
            .any(|zeile| zeile.thread_id == strang.thread_id && zeile.anzahl >= 2)
        {
            angekommen = true;
            break;
        }
    }
    assert!(angekommen, "die Antwort kam nicht am Gespräch an");

    // 6. Und die Kopie liegt in „Gesendet" — gefunden über die Rolle.
    kern.abgleichen(gesendet.id.clone()).await.expect("Gesendet abgleichen");
    let kopien_nachher: u32 = kern
        .threads(gesendet.id.clone(), None, 200)
        .await
        .expect("Liste")
        .zeilen
        .iter()
        .map(|zeile| zeile.anzahl)
        .sum();
    assert_eq!(kopien_nachher, kopien_vorher + 1, "die Kopie fehlt in Gesendet");

    println!("Rundlauf vollständig: ablegen, lesen, antworten, zurückbekommen, Kopie abgelegt.");
}
