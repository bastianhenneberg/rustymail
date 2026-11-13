//! IMAP gegen einen echten Server — GreenMail im Container.
//!
//! Warum nicht gegen eines der 16 Produktiv-Postfächer: Dort liegt Kundenpost.
//! Ein Test, der verschiebt und Flaggen setzt, hat darin nichts zu suchen, und
//! ein Test, der nur liest, prüft die halbe Schnittstelle.
//!
//! Alle Tests hier sind `#[ignore]`: Sie brauchen den Container. Ein Test, der
//! ohne ihn rot ist, wird irgendwann abgeschaltet statt gelesen.
//!
//! ```sh
//! docker run -d --name rustymail-greenmail -p 3025:3025 -p 3143:3143 -p 3993:3993 \
//!   -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.users=test:geheim@localhost -Dgreenmail.hostname=0.0.0.0' \
//!   greenmail/standalone:latest
//! cargo test -p mail-proto --features test-support --test imap_gegen_greenmail -- --ignored --nocapture
//! ```

use mail_proto::imap::ImapBackend;
use mail_proto::{Flagge, Kontobackend, Verschluesselung, Zugang};

fn zugang() -> Zugang {
    Zugang {
        host: "localhost".to_owned(),
        port: 3993,
        // GreenMail legt den Nutzer als test@localhost an, der LOGIN-Name ist
        // aber „test“ — mit der Adresse als Namen lehnt es ab.
        benutzer: "test".to_owned(),
        geheimnis: "geheim".to_owned(),
        verschluesselung: Verschluesselung::Tls,
        anmeldeart: mail_proto::Anmeldeart::Passwort,
        senden: Some(mail_proto::Sendeziel {
            host: "localhost".to_owned(),
            port: 3465,
            verschluesselung: Verschluesselung::Tls,
        }),
    }
}

fn backend() -> ImapBackend {
    // Ungeprüftes Zertifikat: GreenMail stellt sich sein eigenes aus. Diese
    // Tür existiert nur mit `--features test-support`.
    ImapBackend::neu_ungeprueft(zugang())
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn ordner_kommen_an() {
    let backend = backend();
    let ordner = backend.ordner().await.expect("Ordnerliste");

    assert!(!ordner.is_empty(), "kein einziger Ordner");
    assert!(
        ordner.iter().any(|o| o.pfad.eq_ignore_ascii_case("INBOX")),
        "INBOX fehlt: {:?}",
        ordner.iter().map(|o| &o.pfad).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn kopfzeilen_kommen_entschluesselt_an() {
    let backend = backend();
    let seite = backend.kopfzeilen("INBOX", None, 50).await.expect("Kopfzeilen");

    assert!(seite.kopfzeilen.len() >= 3, "zu wenige Nachrichten: {}", seite.kopfzeilen.len());

    // Neueste zuerst — darauf verlässt sich die Oberfläche.
    let uids: Vec<u32> = seite.kopfzeilen.iter().filter_map(|k| k.kennung.parse().ok()).collect();
    let mut absteigend = uids.clone();
    absteigend.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(uids, absteigend, "Reihenfolge ist nicht absteigend");

    // Der eigentliche Prüfpunkt: Der Betreff kam als `=?utf-8?q?…?=` über die
    // Leitung. Steht hier noch ein `=?`, ist die Kopfzeile nicht dekodiert —
    // und genau das sieht man in der Oberfläche als Kauderwelsch.
    let betreffs: Vec<&str> = seite.kopfzeilen.iter().map(|k| k.betreff.as_str()).collect();
    assert!(
        betreffs.iter().any(|b| b.contains("Grünflächenpflege")),
        "Umlaut-Betreff nicht dekodiert: {betreffs:?}"
    );
    assert!(
        !betreffs.iter().any(|b| b.contains("=?")),
        "Kopfzeile ist roh durchgereicht: {betreffs:?}"
    );

    // Absender, Datum und Grösse müssen gefüllt sein, sonst sortiert die
    // Liste nach 1970.
    for zeile in &seite.kopfzeilen {
        assert!(!zeile.von.is_empty(), "Absender fehlt bei UID {}", zeile.kennung);
        assert!(zeile.gesendet_am > 0, "Datum fehlt bei UID {}", zeile.kennung);
        assert!(zeile.groesse > 0, "Grösse fehlt bei UID {}", zeile.kennung);
    }

    // Die Antwort trägt References — daran hängt später die Gesprächsbildung.
    // Gezielt DIESE Antwort, nicht „die erste mit Re:". Im Postfach liegen
    // inzwischen mehrere Antworten (andere Tests senden welche), und ein
    // Test, der sich die erstbeste greift, prüft je nach Reihenfolge etwas
    // anderes — und schlägt irgendwann ohne erkennbaren Grund fehl.
    let antwort = seite
        .kopfzeilen
        .iter()
        .find(|k| k.betreff.starts_with("Re: ") && k.betreff.contains("Grünflächenpflege"))
        .expect("die Antwort auf das Angebot fehlt");
    assert_eq!(antwort.in_reply_to.as_deref(), Some("erste@beispiel.de"));
    assert!(
        antwort.references.iter().any(|r| r.contains("erste@beispiel.de")),
        "References leer: {:?}",
        antwort.references
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn seitenweise_blaettern_ueberschneidet_sich_nicht() {
    let backend = backend();

    let erste = backend.kopfzeilen("INBOX", None, 2).await.expect("erste Seite");
    assert_eq!(erste.kopfzeilen.len(), 2);
    let marke = erste.weiter_ab.clone().expect("es muss weitergehen");

    let zweite = backend
        .kopfzeilen("INBOX", Some(&marke), 2)
        .await
        .expect("zweite Seite");

    let erste_kennungen: Vec<&str> = erste
        .kopfzeilen
        .iter()
        .map(|k| k.kennung.as_str())
        .collect();
    for zeile in &zweite.kopfzeilen {
        assert!(
            !erste_kennungen.contains(&zeile.kennung.as_str()),
            "Kennung {} steht auf beiden Seiten",
            zeile.kennung
        );
    }
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn die_rohfassung_ist_eine_ganze_nachricht() {
    let backend = backend();
    let seite = backend.kopfzeilen("INBOX", None, 50).await.expect("Kopfzeilen");
    let mit_anhang = seite
        .kopfzeilen
        .iter()
        .find(|k| k.betreff.contains("Rechnung"))
        .expect("Rechnung nicht gefunden");

    let rohdaten = backend
        .rohfassung("INBOX", &mit_anhang.kennung)
        .await
        .expect("Rohfassung");

    let text = String::from_utf8_lossy(&rohdaten);
    assert!(text.contains("Message-ID"), "keine Kopfzeilen in der Rohfassung");
    assert!(text.contains("zahlen.csv"), "der Anhang fehlt");
    assert!(
        rohdaten.len() as u32 >= mit_anhang.groesse / 2,
        "Rohfassung ist verdächtig kurz: {} statt ~{}",
        rohdaten.len(),
        mit_anhang.groesse
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn lesen_setzt_nicht_gelesen() {
    let backend = backend();

    // Zweimal dieselbe Nachricht holen. Wer mit SELECT und BODY[] statt
    // EXAMINE und BODY.PEEK[] arbeitet, dreht beim ersten Mal ungelesen auf
    // gelesen — und bemerkt es nie, weil die Liste danach stimmt.
    let vorher = backend.kopfzeilen("INBOX", None, 1).await.expect("Kopfzeilen");
    let uid = vorher.kopfzeilen[0].kennung.clone();

    // Den Ausgangszustand SELBST herstellen, statt ihn vorauszusetzen.
    // Andere Tests und die Anwendung setzen in diesem Postfach Flaggen; ein
    // Test, der „ist bestimmt noch ungelesen" annimmt, ist irgendwann rot
    // und sagt dann nichts über den Code.
    backend
        .flaggen_setzen("INBOX", &uid, &[], &[Flagge::Gelesen])
        .await
        .expect("Ausgangszustand herstellen");

    backend.rohfassung("INBOX", &uid).await.expect("Rohfassung");

    let nachher = backend.kopfzeilen("INBOX", None, 1).await.expect("Kopfzeilen");
    assert!(
        !nachher.kopfzeilen[0].flaggen.contains(&Flagge::Gelesen),
        "Das Öffnen hat \\Seen gesetzt"
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn flaggen_setzen_und_wieder_wegnehmen() {
    let backend = backend();
    let seite = backend.kopfzeilen("INBOX", None, 1).await.expect("Kopfzeilen");
    let uid = seite.kopfzeilen[0].kennung.clone();

    backend
        .flaggen_setzen("INBOX", &uid, &[Flagge::Markiert], &[])
        .await
        .expect("setzen");

    let markiert = backend.kopfzeilen("INBOX", None, 1).await.expect("Kopfzeilen");
    assert!(
        markiert.kopfzeilen[0].flaggen.contains(&Flagge::Markiert),
        "Flagge kam nicht an: {:?}",
        markiert.kopfzeilen[0].flaggen
    );

    backend
        .flaggen_setzen("INBOX", &uid, &[], &[Flagge::Markiert])
        .await
        .expect("wegnehmen");

    let sauber = backend.kopfzeilen("INBOX", None, 1).await.expect("Kopfzeilen");
    assert!(
        !sauber.kopfzeilen[0].flaggen.contains(&Flagge::Markiert),
        "Flagge blieb stehen"
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn append_uebernimmt_das_mitgegebene_datum() {
    let backend = backend();

    // Bewusst OHNE Date-Kopfzeile: Dann ist das Datum, das die Liste zeigt,
    // allein das internaldate des Servers — und genau das soll APPEND
    // setzen. Mit Date-Kopfzeile würde der Test an ihr vorbeiprüfen.
    let rohdaten = b"From: alt@beispiel.de\r\n\
To: test@localhost\r\n\
Subject: Aus dem Archiv\r\n\
Message-ID: <archiv-pruefung@beispiel.de>\r\n\
\r\n\
Eine alte Nachricht.\r\n";

    // 1. Juli 2021, 12:00:00 UTC.
    let damals: i64 = 1_625_140_800_000;

    backend
        .anhaengen("Gesendet", rohdaten, damals, &[Flagge::Gelesen])
        .await
        .expect("APPEND");

    let seite = backend
        .kopfzeilen("Gesendet", None, 50)
        .await
        .expect("Kopfzeilen");

    let eingelegt = seite
        .kopfzeilen
        .iter()
        .find(|zeile| zeile.betreff == "Aus dem Archiv")
        .expect("die eingelegte Nachricht fehlt");

    // Der eigentliche Prüfpunkt. Ohne internaldate stünde hier der Zeitpunkt
    // des Einlegens — also heute — und die Nachricht rutschte in einer nach
    // Datum sortierten Liste nach oben, als wäre sie neu.
    let abweichung = (eingelegt.gesendet_am - damals).abs();
    assert!(
        abweichung < 1000,
        "Datum nicht übernommen: erwartet {damals}, bekommen {} (Abweichung {abweichung} ms)",
        eingelegt.gesendet_am
    );

    assert!(
        eingelegt.flaggen.contains(&Flagge::Gelesen),
        "die Flagge kam nicht mit: {:?}",
        eingelegt.flaggen
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn jede_uid_traegt_ihre_eigenen_kopfzeilen() {
    // Der Prüfpunkt, der bisher gefehlt hat: Die bisherigen Tests prüften,
    // DASS Felder gefüllt sind — nicht, dass sie zur richtigen Nachricht
    // gehören. Eine Verschiebung um eine Stelle fällt dabei nicht auf.
    let backend = backend();
    let seite = backend.kopfzeilen("INBOX", None, 100).await.expect("Kopfzeilen");

    let mut gesehen: Vec<(u32, String, Option<String>)> = seite
        .kopfzeilen
        .iter()
        .filter_map(|zeile| {
            Some((
                zeile.kennung.parse().ok()?,
                zeile.betreff.clone(),
                zeile.message_id.clone(),
            ))
        })
        .collect();
    gesehen.sort_by_key(|(uid, _, _)| *uid);

    for (uid, betreff, message_id) in &gesehen {
        println!("uid={uid} betreff={betreff:?} id={message_id:?}");
    }

    // Keine Kennung darf doppelt vorkommen — zwei UIDs mit derselben
    // Message-Id heisst, dass eine Antwort die Kopfzeilen ihrer Nachbarin
    // bekommen hat.
    let mut kennungen: Vec<&String> = gesehen
        .iter()
        .filter_map(|(_, _, id)| id.as_ref())
        .collect();
    let vorher = kennungen.len();
    kennungen.sort();
    kennungen.dedup();
    assert_eq!(
        kennungen.len(),
        vorher,
        "zwei UIDs tragen dieselbe Message-Id: {gesehen:?}"
    );

    // Und die Testpost muss vollständig sein.
    let betreffs: Vec<&str> = gesehen.iter().map(|(_, b, _)| b.as_str()).collect();
    for erwartet in ["Notiz an mich", "Rechnung 2026-1147"] {
        assert!(
            betreffs.iter().any(|b| b.contains(erwartet)),
            "{erwartet:?} fehlt: {betreffs:?}"
        );
    }
}


// XOAUTH2 lässt sich hier NICHT prüfen: GreenMail verlangt die Anmeldedaten
// in derselben Zeile (SASL-IR), `async-imap` schickt sie erst nach der
// Aufforderung des Servers. Beide Formen sind zulässig, Gmail und Outlook
// können beide — GreenMail nur die eine.
//
// Ein Test dagegen wäre zudem aus dem falschen Grund grün gewesen: GreenMails
// Ablehnung „Missing argument" kommt als `AnmeldungAbgelehnt` an, genau wie
// ein abgelaufenes Token. Er hätte bestanden, ohne irgendetwas zu zeigen.
//
// Geprüft wird XOAUTH2 deshalb in `xoauth2_gegen_stub.rs` gegen einen
// eigenen Server, der mitschreibt, was wirklich über die Leitung geht.
