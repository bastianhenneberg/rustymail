//! JMAP gegen einen echten Server — Stalwart im Container.
//!
//! GreenMail spricht kein JMAP; dieselbe Testpost lässt sich dort also nicht
//! verwenden. Stalwart bringt beides mit und ist damit der Gegenprobe-Server
//! für die zweite Erfüllung desselben Vertrags.
//!
//! ```sh
//! docker run -d --name rustymail-stalwart -p 18080:8080 \
//!   -v rustymail-stalwart-daten:/opt/stalwart stalwartlabs/stalwart:latest
//! # Das Startkennwort steht EINMALIG im Protokoll:
//! docker logs rustymail-stalwart | grep -A3 password
//! RUSTYMAIL_STALWART_PASSWORT=… cargo test -p mail-proto --test jmap_gegen_stalwart -- --ignored
//! ```

use mail_proto::jmap::JmapBackend;
use mail_proto::{Flagge, Kontobackend};

fn backend() -> JmapBackend {
    // Das Kennwort steht nicht im Quelltext: Es wird beim ersten Start des
    // Containers erzeugt und ist bei jedem neuen Container ein anderes.
    let passwort = std::env::var("RUSTYMAIL_STALWART_PASSWORT").expect(
        "RUSTYMAIL_STALWART_PASSWORT setzen — siehe Kopf dieser Datei",
    );
    JmapBackend::neu(
        "http://localhost:18080".to_owned(),
        "admin".to_owned(),
        passwort,
    )
}

#[tokio::test]
#[ignore = "braucht den Stalwart-Container"]
async fn die_sitzung_findet_konto_und_ordner() {
    let backend = backend();
    let ordner = backend.ordner().await.expect("Ordnerliste");

    assert!(!ordner.is_empty(), "keine Ordner");

    // Die Kennung ist die Mailbox-Id, NICHT der Name: Sie bleibt gleich,
    // wenn jemand den Ordner umbenennt.
    let eingang = ordner
        .iter()
        .find(|o| o.rolle.as_deref() == Some("inbox"))
        .expect("kein Eingang mit Rolle `inbox`");
    assert!(!eingang.pfad.is_empty());
    assert_ne!(
        eingang.pfad, eingang.name,
        "Kennung und Name sind dasselbe — dann ist die Id nicht durchgereicht"
    );

    // Die Rollen heissen wie bei IMAP — darauf baut der Kern auf.
    let rollen: Vec<&str> = ordner.iter().filter_map(|o| o.rolle.as_deref()).collect();
    for erwartet in ["inbox", "sent", "trash"] {
        assert!(rollen.contains(&erwartet), "Rolle {erwartet} fehlt: {rollen:?}");
    }
}

#[tokio::test]
#[ignore = "braucht den Stalwart-Container"]
async fn kopfzeilen_kommen_mit_kennung_datum_und_bezug() {
    let backend = backend();
    let ordner = backend.ordner().await.expect("Ordnerliste");
    let eingang = ordner
        .iter()
        .find(|o| o.rolle.as_deref() == Some("inbox"))
        .expect("Eingang");

    let seite = backend
        .kopfzeilen(&eingang.pfad, None, 50)
        .await
        .expect("Kopfzeilen");

    assert!(!seite.kopfzeilen.is_empty(), "keine Nachricht im Eingang");
    let erste = &seite.kopfzeilen[0];

    // Eine JMAP-Kennung ist KEINE Zahl. Genau dafür wurde der Vertrag auf
    // Zeichenketten umgestellt.
    assert!(!erste.kennung.is_empty());
    assert!(
        erste.kennung.parse::<u32>().is_err(),
        "die Kennung sieht aus wie eine UID — dann stimmt der Vertrag nicht: {}",
        erste.kennung
    );

    // Der Betreff muss dekodiert sein — er kam als =?utf-8?q?…?= an.
    assert!(
        erste.betreff.contains("Grünflächenpflege"),
        "Betreff nicht dekodiert: {:?}",
        erste.betreff
    );
    assert!(!erste.von.is_empty(), "kein Absender");
    assert!(erste.gesendet_am > 0, "kein Zeitpunkt — RFC-3339 nicht gelesen");
    assert!(erste.groesse > 0, "keine Grösse");
}

#[tokio::test]
#[ignore = "braucht den Stalwart-Container"]
async fn die_rohfassung_kommt_ueber_den_blob() {
    let backend = backend();
    let ordner = backend.ordner().await.expect("Ordnerliste");
    let eingang = ordner
        .iter()
        .find(|o| o.rolle.as_deref() == Some("inbox"))
        .expect("Eingang");
    let seite = backend
        .kopfzeilen(&eingang.pfad, None, 1)
        .await
        .expect("Kopfzeilen");

    let rohdaten = backend
        .rohfassung(&eingang.pfad, &seite.kopfzeilen[0].kennung)
        .await
        .expect("Rohfassung");

    let text = String::from_utf8_lossy(&rohdaten);
    assert!(text.contains("Message-ID"), "keine Kopfzeilen: {}", &text[..text.len().min(120)]);
    assert!(text.contains("Angebot anpassen"), "kein Inhalt");
}

#[tokio::test]
#[ignore = "braucht den Stalwart-Container"]
async fn flaggen_setzen_und_wieder_wegnehmen() {
    let backend = backend();
    let ordner = backend.ordner().await.expect("Ordnerliste");
    let eingang = ordner
        .iter()
        .find(|o| o.rolle.as_deref() == Some("inbox"))
        .expect("Eingang");
    let seite = backend
        .kopfzeilen(&eingang.pfad, None, 1)
        .await
        .expect("Kopfzeilen");
    let kennung = seite.kopfzeilen[0].kennung.clone();

    // Ausgangszustand selbst herstellen, nicht voraussetzen.
    backend
        .flaggen_setzen(&eingang.pfad, &kennung, &[], &[Flagge::Markiert])
        .await
        .expect("zurücksetzen");

    backend
        .flaggen_setzen(&eingang.pfad, &kennung, &[Flagge::Markiert], &[])
        .await
        .expect("setzen");

    let markiert = backend
        .kopfzeilen(&eingang.pfad, None, 1)
        .await
        .expect("Kopfzeilen");
    assert!(
        markiert.kopfzeilen[0].flaggen.contains(&Flagge::Markiert),
        "Schlüsselwort kam nicht an: {:?}",
        markiert.kopfzeilen[0].flaggen
    );

    backend
        .flaggen_setzen(&eingang.pfad, &kennung, &[], &[Flagge::Markiert])
        .await
        .expect("wegnehmen");

    let sauber = backend
        .kopfzeilen(&eingang.pfad, None, 1)
        .await
        .expect("Kopfzeilen");
    assert!(
        !sauber.kopfzeilen[0].flaggen.contains(&Flagge::Markiert),
        "Schlüsselwort blieb stehen"
    );
}
