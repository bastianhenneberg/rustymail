//! XOAUTH2 gegen den echten Stalwart.
//!
//! Warum nicht gegen GreenMail: Der verlangt die Anmeldedaten in derselben
//! Zeile (SASL-IR), `async-imap` schickt sie erst nach der Aufforderung.
//! Stalwart beherrscht beide Formen — gemessen am 12.11.2026: Auf
//! `AUTHENTICATE XOAUTH2` ohne Daten antwortet er mit `+ `.
//!
//! Warum nicht gegen eine Attrappe: Ein selbstgebauter IMAP-Server prüft am
//! Ende das eigene Verständnis des Protokolls, nicht das des Servers. Die
//! erste Fassung dieses Tests tat genau das — und blieb hängen.
//!
//! **Was dieser Test zeigt:** TLS gegen ein echtes Zertifikat, die
//! Aushandlung von `AUTHENTICATE XOAUTH2`, die Behandlung der leeren
//! Aufforderung und die Übersetzung der Ablehnung in `AnmeldungAbgelehnt`.
//! **Was er nicht zeigen kann:** eine erfolgreiche Anmeldung — dafür braucht
//! es ein echtes OAuth-Token, und das setzt eine registrierte Anwendung bei
//! Google beziehungsweise Microsoft voraus.
//!
//! ```sh
//! RUSTYMAIL_IMAP_HOST=mail.example.de RUSTYMAIL_IMAP_USER=post@example.de \
//!   cargo test -p mail-proto --test xoauth2_gegen_stalwart -- --ignored
//! ```

use mail_proto::imap::ImapBackend;
use mail_proto::{Anmeldeart, Kontobackend, Verschluesselung, Zugang};

fn zugang(geheimnis: &str, anmeldeart: Anmeldeart) -> Zugang {
    Zugang {
        host: std::env::var("RUSTYMAIL_IMAP_HOST")
            .expect("RUSTYMAIL_IMAP_HOST setzen — siehe Kopf dieser Datei"),
        port: 993,
        benutzer: std::env::var("RUSTYMAIL_IMAP_USER")
            .expect("RUSTYMAIL_IMAP_USER setzen"),
        geheimnis: geheimnis.to_owned(),
        verschluesselung: Verschluesselung::Tls,
        anmeldeart,
        senden: None,
    }
}

#[tokio::test]
#[ignore = "braucht einen erreichbaren echten IMAP-Server"]
async fn ein_ungueltiges_token_kommt_als_anmeldefehler_an() {
    // Der Weg bis hierher ist der eigentliche Prüfpunkt: TLS mit
    // Zertifikatsprüfung gegen den Systemspeicher, `AUTHENTICATE XOAUTH2`,
    // die leere Aufforderung, die Antwort im vorgeschriebenen Format — und
    // erst ganz am Ende die Ablehnung.
    //
    // Für die Oberfläche zählt die ART des Fehlers: Nur bei
    // `AnmeldungAbgelehnt` zeigt sie „neu anmelden" statt „später nochmal
    // versuchen". Ein abgelaufenes Token sieht für den Server genauso aus
    // wie dieses erfundene.
    let backend = ImapBackend::neu(zugang("kein-gueltiges-token", Anmeldeart::XOauth2));

    let fehler = backend.ordner().await.expect_err("muss scheitern");
    assert!(
        matches!(fehler, mail_proto::Protokollfehler::AnmeldungAbgelehnt(_)),
        "falsche Fehlerart — die Oberfläche zeigt dann das Falsche an: {fehler:?}"
    );
}

#[tokio::test]
#[ignore = "braucht einen erreichbaren echten IMAP-Server"]
async fn ein_falsches_passwort_wird_anders_gemeldet_als_ein_ausfall() {
    // Gegenprobe zur Passwort-Anmeldung, damit beide Wege dieselbe
    // Fehlerart liefern.
    let backend = ImapBackend::neu(zugang("falsch", Anmeldeart::Passwort));

    let fehler = backend.ordner().await.expect_err("muss scheitern");
    assert!(
        matches!(fehler, mail_proto::Protokollfehler::AnmeldungAbgelehnt(_)),
        "falsche Fehlerart: {fehler:?}"
    );
}
