//! STARTTLS beim Senden.
//!
//! Viele Anbieter bieten den Postausgang **nur** auf 587 mit STARTTLS an —
//! Hetzners Webhosting etwa, über das die Peppermint-Anwendungen heute
//! versenden. Ohne diesen Weg fehlt der Client dort ganz.
//!
//! **Was hier geprüft wird:** dass aus einer Klartextverbindung eine
//! verschlüsselte wird und das Zertifikat dabei gegen den Vertrauensspeicher
//! des Systems geprüft wird.
//!
//! **Was nicht geprüft werden kann:** die Anmeldung darüber. Erreichbar ist
//! hier nur Port 25, und der bietet nach dem Aufstieg kein `AUTH` an — er ist
//! der Empfangsweg, nicht der Einlieferungsweg. Gemessen am 12.11.2026:
//! `EHLO` listet `starttls`, nach dem Aufstieg fehlt `auth`. Für den vollen
//! Nachweis braucht es einen Einlieferungsweg auf 587.
//!
//! ```sh
//! RUSTYMAIL_SMTP_HOST=mail.example.de \
//!   cargo test -p mail-proto --test starttls_gegen_stalwart -- --ignored
//! ```

use mail_proto::imap::ImapBackend;
use mail_proto::{Anmeldeart, Kontobackend, Sendeziel, Verschluesselung, Zugang};

fn backend(port: u16, verschluesselung: Verschluesselung) -> ImapBackend {
    let host = std::env::var("RUSTYMAIL_SMTP_HOST")
        .expect("RUSTYMAIL_SMTP_HOST setzen — siehe Kopf dieser Datei");

    ImapBackend::neu(Zugang {
        // Der Posteingang wird hier nicht angefasst; geprüft wird allein der
        // Sendeweg.
        host: host.clone(),
        port: 993,
        benutzer: "probe@beispiel.de".to_owned(),
        geheimnis: "egal".to_owned(),
        verschluesselung: Verschluesselung::Tls,
        anmeldeart: Anmeldeart::Passwort,
        senden: Some(Sendeziel { host, port, verschluesselung }),
    })
}

#[tokio::test]
#[ignore = "braucht einen erreichbaren Mailserver"]
async fn starttls_steigt_auf_und_scheitert_erst_an_der_anmeldung() {
    // Es wird NICHTS verschickt: Der Versuch bricht vor `DATA` ab, weil die
    // Anmeldung fehlschlägt. Geprüft ist damit alles davor — Verbindung,
    // EHLO, der Aufstieg nach TLS und die Zertifikatsprüfung.
    let backend = backend(25, Verschluesselung::Starttls);

    let fehler = backend
        .senden(b"", "probe@beispiel.de", &["ziel@beispiel.de".to_owned()])
        .await
        .expect_err("ohne Anmeldung muss es scheitern");

    // Der Prüfpunkt: KEIN `NichtErreichbar` mit TLS-Grund. Wäre der Aufstieg
    // gescheitert, stünde hier ein Zertifikats- oder Handschlagfehler.
    let text = fehler.to_string();
    assert!(
        !text.contains("certificate") && !text.contains("TLS:"),
        "der STARTTLS-Aufstieg ist gescheitert: {text}"
    );
}

#[tokio::test]
#[ignore = "braucht einen erreichbaren Mailserver"]
async fn implizites_tls_auf_einem_klartext_port_scheitert_sichtbar() {
    // Gegenprobe. Wer 587 mit implizitem TLS einträgt — ein häufiger
    // Konfigurationsfehler —, bekommt keine stille Klartextverbindung,
    // sondern einen Fehler.
    let backend = backend(25, Verschluesselung::Tls);

    let fehler = backend
        .senden(b"", "probe@beispiel.de", &["ziel@beispiel.de".to_owned()])
        .await
        .expect_err("implizites TLS auf einem Klartext-Port kann nicht gehen");

    println!("erwarteter Fehler: {fehler}");
}
