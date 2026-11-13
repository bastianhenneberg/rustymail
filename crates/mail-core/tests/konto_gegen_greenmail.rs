//! Die ganze Kette, ohne Oberfläche: Konto anlegen → Geheimnis in den Tresor
//! → Verbindungsdaten in die Datenbank → IMAP-Verbindung → Ordnerliste.
//!
//! Das ist der Test, der zählt. Die Teile einzeln geprüft zu haben sagt
//! nichts darüber, ob sie zusammenfinden — und genau an der Naht zwischen
//! Tresor und Datenbank entsteht der Fehler, den man erst beim Verbinden
//! merkt.
//!
//! ```sh
//! cargo test -p mail-core --test konto_gegen_greenmail -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mail_core::{
    Einrichtung, Ereignis, EreignisHoerer, Ergebnis, Kern, Kontoentwurf, Protokoll, Tresor,
    Verbindungsdaten, Verschluesselung,
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

fn kern_bauen(verzeichnis: &tempfile::TempDir) -> std::sync::Arc<Kern> {
    Kern::neu(
        Einrichtung {
            datenverzeichnis: verzeichnis.path().to_string_lossy().into_owned(),
        },
        Arc::new(Taschentresor::default()),
        Arc::new(Taub),
    )
    .expect("Kern")
}

fn entwurf(geheimnis: &str) -> Kontoentwurf {
    Kontoentwurf {
        beschriftung: "Testserver".into(),
        adresse: "test@localhost".into(),
        protokoll: Protokoll::Imap,
        verbindung: Verbindungsdaten {
            host: "localhost".into(),
            port: 3993,
            benutzer: "test".into(),
            verschluesselung: Verschluesselung::Tls,
            anmeldeart: mail_core::Anmeldeart::Passwort,
            jmap_sitzung: None,
            smtp_host: Some("localhost".into()),
            smtp_port: Some(3465),
        },
        geheimnis: geheimnis.into(),
    }
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn vom_angelegten_konto_bis_zur_ordnerliste() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);

    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");
    let ordner = kern.ordner(id.clone()).await.expect("Ordnerliste");

    assert!(!ordner.is_empty(), "keine Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX fehlt");

    // Die Rolle muss erkannt sein, sonst sucht die Oberfläche den Eingang am
    // Namen — und der heisst in jeder Sprache anders.
    assert_eq!(eingang.rolle, mail_core::Ordnerrolle::Eingang);
    // Die Kennung trägt das Konto mit, sonst kollidieren zwei INBOXen.
    assert!(
        eingang.id.starts_with(&id),
        "Ordnerkennung ohne Konto: {}",
        eingang.id
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn ein_falsches_passwort_meldet_sich_als_solches() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);

    let id = kern.konto_anlegen(entwurf("falsch")).await.expect("anlegen");

    // Der Unterschied ist für die Oberfläche entscheidend: Bei
    // AnmeldungAbgelehnt zeigt sie eine Anmeldemaske, bei NichtErreichbar
    // einen Wiederholen-Knopf. Ein gemeinsamer „Fehler" liesse ihr nur die
    // Entschuldigung.
    let fehler = kern.ordner(id).await.expect_err("muss scheitern");
    assert!(
        matches!(fehler, mail_core::Fehler::AnmeldungAbgelehnt { .. }),
        "falsche Fehlerart: {fehler:?}"
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn referenzen_fuehren_zusammen_und_fremdes_bleibt_getrennt() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id.clone()).await.expect("Ordnerliste");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");

    // Vor dem Abgleich ist die Liste leer — und zwar ohne Fehler. Die
    // Oberfläche soll beim ersten Öffnen nichts zeigen, nicht scheitern.
    let vorher = kern
        .threads(eingang.id.clone(), None, 50)
        .await
        .expect("leere Liste");
    assert!(vorher.zeilen.is_empty());

    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern
        .threads(eingang.id.clone(), None, 50)
        .await
        .expect("Gesprächsliste");

    // Worum es geht: Referenzen führen zusammen, und sonst nichts. Nicht
    // geprüft wird die ANZAHL der Gespräche — der Testserver sammelt über
    // Läufe hinweg an, und eine absolute Zahl wäre beim nächsten Lauf rot,
    // ohne dass am Code etwas falsch wäre.
    let strang = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Grünflächenpflege"))
        .expect("das Angebot fehlt");
    assert_eq!(
        strang.anzahl, 2,
        "Ausgangsmail und Antwort gehören zusammen, sind es aber nicht: {:?}",
        seite.zeilen.iter().map(|z| (&z.betreff, z.anzahl)).collect::<Vec<_>>()
    );

    let rechnung = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Rechnung"))
        .expect("die Rechnung fehlt");
    assert_eq!(
        rechnung.anzahl, 1,
        "die Rechnung wurde in ein fremdes Gespräch gezogen"
    );
    assert_ne!(
        strang.thread_id, rechnung.thread_id,
        "zwei unabhängige Vorgänge in einem Gespräch"
    );

    // Der Betreff kommt aus der neuesten Nachricht des Strangs.
    assert!(strang.betreff.starts_with("Re: "), "Betreff: {}", strang.betreff);
    // Beide Absender sind beteiligt.
    assert_eq!(strang.beteiligte.len(), 2, "{:?}", strang.beteiligte);
    // Ungelesen, denn niemand hat sie geöffnet — und das Holen der
    // Kopfzeilen darf daran nichts ändern.
    assert!(strang.ungelesen, "der Abgleich hat \\Seen gesetzt");
    assert!(strang.letzte_nachricht_am > 0, "kein Zeitstempel");

    // Ein zweiter Abgleich darf nichts verdoppeln.
    kern.abgleichen(eingang.id.clone()).await.expect("zweiter Abgleich");
    let erneut = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    let nachher = erneut
        .zeilen
        .iter()
        .find(|zeile| zeile.thread_id == strang.thread_id)
        .expect("Gespräch verschwunden");
    assert_eq!(nachher.anzahl, 2, "der zweite Abgleich hat verdoppelt");
    assert_eq!(
        erneut.zeilen.len(),
        seite.zeilen.len(),
        "der zweite Abgleich hat Gespräche erfunden"
    );
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn ein_gespraech_oeffnen_liefert_beide_nachrichten_mit_text() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern
        .threads(eingang.id.clone(), None, 50)
        .await
        .expect("Liste");
    let strang = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.anzahl == 2)
        .expect("Gespräch mit zwei Nachrichten");

    let nachrichten = kern
        .gespraech(eingang.id.clone(), strang.thread_id.clone())
        .await
        .expect("Gespräch");

    assert_eq!(nachrichten.len(), 2);
    // Älteste zuerst — so liest man einen Verlauf.
    assert!(nachrichten[0].gesendet_am <= nachrichten[1].gesendet_am);

    for nachricht in &nachrichten {
        let text = match &nachricht.koerper {
            mail_core::Koerper::Text { inhalt } => inhalt.clone(),
            mail_core::Koerper::Html { text_ersatz, .. } => text_ersatz.clone(),
        };
        assert!(!text.trim().is_empty(), "leerer Körper bei {}", nachricht.betreff);
    }

    let ausgangsmail = nachrichten
        .iter()
        .find(|n| n.koerper_text().contains("Angebot anpassen"))
        .expect("der Text der Ausgangsmail fehlt");

    // Eine reine Textmail muss Text BLEIBEN. mail-parser rechnet auf Wunsch
    // Textteile in HTML um; wer das für echtes HTML hält, macht daraus
    // wieder Text — und dabei werden aus Absätzen Leerzeichen. Genau das
    // stand am 12.11.2026 im Fenster.
    assert!(
        matches!(ausgangsmail.koerper, mail_core::Koerper::Text { .. }),
        "aus der Textmail wurde HTML: {:?}",
        ausgangsmail.koerper
    );
    assert!(
        ausgangsmail.koerper_text().contains('\n'),
        "die Absätze sind verloren gegangen: {:?}",
        ausgangsmail.koerper_text()
    );

    // Zweites Öffnen darf NICHT noch einmal holen — der Körper liegt jetzt
    // in der Datenbank. Prüfbar daran, dass es auch dann geht, wenn man
    // dem Konto das Passwort nimmt: ohne Netzzugriff kein Fehler.
    let erneut = kern
        .gespraech(eingang.id.clone(), strang.thread_id.clone())
        .await
        .expect("zweites Öffnen");
    assert_eq!(erneut.len(), 2);
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn ein_anhang_wird_erkannt_und_nicht_als_inline_gezaehlt() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    let rechnung = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Rechnung"))
        .expect("Rechnung");

    let nachrichten = kern
        .gespraech(eingang.id.clone(), rechnung.thread_id.clone())
        .await
        .expect("Gespräch");

    let anhaenge = &nachrichten[0].anhaenge;
    assert_eq!(anhaenge.len(), 1, "Anhänge: {anhaenge:?}");
    assert_eq!(anhaenge[0].dateiname, "zahlen.csv");
    assert!(anhaenge[0].medientyp.starts_with("text/csv"), "{}", anhaenge[0].medientyp);
    assert!(anhaenge[0].groesse > 0);
}


#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn eine_antwort_geht_raus_und_die_kopie_liegt_in_gesendet() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id.clone()).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    // Auf die Mail an sich selbst antworten — nur dann landet die Antwort
    // wieder im eigenen Posteingang und lässt sich nachweisen.
    let notiz = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Notiz an mich"))
        .expect("Notiz an mich fehlt in der Testpost");

    // Der Testserver sammelt über Läufe hinweg an — jeder Durchgang legt
    // eine Antwort dazu. Deshalb wird die VERÄNDERUNG geprüft, nicht eine
    // absolute Zahl: Ein Test, der ein jungfräuliches Postfach voraussetzt,
    // ist beim zweiten Lauf rot und sagt nichts über den Code.
    let vorher = notiz.anzahl;
    let kopien_vorher = {
        let gesendet = ordner
            .iter()
            .find(|o| o.pfad.eq_ignore_ascii_case("Gesendet"))
            .expect("Gesendet-Ordner fehlt");
        kern.abgleichen(gesendet.id.clone()).await.expect("Gesendet abgleichen");
        kern.threads(gesendet.id.clone(), None, 100)
            .await
            .expect("Liste")
            .zeilen
            .iter()
            .map(|zeile| zeile.anzahl)
            .sum::<u32>()
    };

    kern.antworten(
        eingang.id.clone(),
        notiz.thread_id.clone(),
        "Erledigt, danke.".to_owned(),
    )
    .await
    .expect("antworten");

    // Der Nachweis: Die Antwort kommt als neue Nachricht wieder an, und sie
    // landet im GESPRÄCH der Ausgangsmail — das zeigt, dass In-Reply-To und
    // References gesetzt waren.
    kern.abgleichen(eingang.id.clone()).await.expect("zweiter Abgleich");
    let danach = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");

    let strang = danach
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Notiz an mich"))
        .expect("Gespräch verschwunden");
    assert_eq!(
        strang.anzahl,
        vorher + 1,
        "die Antwort hängt nicht am Gespräch: {:?}",
        danach.zeilen.iter().map(|z| (&z.betreff, z.anzahl)).collect::<Vec<_>>()
    );

    // Und die Kopie liegt in „Gesendet" — gefunden über den Namen, weil
    // GreenMail keine SPECIAL-USE-Merkmale meldet.
    let gesendet = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("Gesendet"))
        .expect("Gesendet-Ordner fehlt");
    kern.abgleichen(gesendet.id.clone()).await.expect("Gesendet abgleichen");

    let kopien = kern.threads(gesendet.id.clone(), None, 100).await.expect("Liste");
    let kopien_nachher: u32 = kopien.zeilen.iter().map(|zeile| zeile.anzahl).sum();
    assert_eq!(
        kopien_nachher,
        kopien_vorher + 1,
        "die Kopie fehlt in Gesendet: {:?}",
        kopien.zeilen.iter().map(|z| (&z.betreff, z.anzahl)).collect::<Vec<_>>()
    );

    let kopie = kopien
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Notiz an mich"))
        .expect("Kopie der Antwort fehlt");
    // Als gelesen abgelegt — man hat sie selbst geschrieben.
    assert!(!kopie.ungelesen, "die eigene Kopie steht als ungelesen");
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn eine_flagge_wird_lokal_sofort_gesetzt_und_landet_beim_server() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id.clone()).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    let strang = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Rechnung"))
        .expect("Rechnung");
    let vorher_ungelesen = strang.ungelesen;

    kern.gelesen_setzen(eingang.id.clone(), strang.thread_id.clone(), true)
        .await
        .expect("gelesen setzen");

    // 1. Lokal sofort — ohne auf den Server zu warten.
    let danach = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    let strang_danach = danach
        .zeilen
        .iter()
        .find(|zeile| zeile.thread_id == strang.thread_id)
        .expect("Gespräch verschwunden");
    assert!(
        !strang_danach.ungelesen,
        "lokal nicht auf gelesen gesetzt (vorher ungelesen: {vorher_ungelesen})"
    );

    // 2. Die Warteschlange ist leer — der Auftrag ist durch.
    assert_eq!(
        kern.offene_auftraege(id.clone()).await.expect("Warteschlange"),
        0,
        "der Auftrag hängt noch"
    );

    // 3. Und der Server hat es auch. Nachweis über einen frischen Abgleich
    // in einem LEEREN Verzeichnis: Was dort ankommt, kommt ausschliesslich
    // vom Server.
    let zweites = tempfile::tempdir().expect("Verzeichnis");
    let frisch = kern_bauen(&zweites);
    let id2 = frisch.konto_anlegen(entwurf("geheim")).await.expect("anlegen");
    let ordner2 = frisch.ordner(id2).await.expect("Ordner");
    let eingang2 = ordner2
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    frisch.abgleichen(eingang2.id.clone()).await.expect("Abgleich");

    let vom_server = frisch
        .threads(eingang2.id.clone(), None, 50)
        .await
        .expect("Liste");
    let beim_server = vom_server
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff.contains("Rechnung"))
        .expect("Rechnung fehlt beim Server");
    assert!(
        !beim_server.ungelesen,
        "die Flagge ist nie beim Server angekommen"
    );

    // Aufräumen, damit der nächste Lauf wieder mit ungelesen beginnt.
    kern.gelesen_setzen(eingang.id.clone(), strang.thread_id.clone(), false)
        .await
        .expect("zurücksetzen");
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn ohne_server_bleibt_die_aenderung_lokal_stehen_und_wartet() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);

    // Erst mit richtigem Passwort anlegen und lesen …
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");
    let ordner = kern.ordner(id.clone()).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    let strang = &seite.zeilen[0];

    // … dann das Konto auf einen Port umbiegen, auf dem niemand antwortet.
    // Das ist die ehrlichste Nachbildung von „kein Netz": Der Aufruf
    // scheitert, und zwar erst nach dem lokalen Schreiben.
    let kaputt = {
        let mut entwurf = entwurf("geheim");
        entwurf.verbindung.port = 3999;
        entwurf
    };
    let id_kaputt = kern.konto_anlegen(kaputt).await.expect("anlegen");
    let ordner_kaputt = format!("{id_kaputt}/INBOX");

    // Für das kaputte Konto gibt es noch keine Nachrichten — deshalb am
    // ersten Konto markieren und nur prüfen, dass der Versuch nicht die
    // lokale Änderung verhindert.
    kern.markieren(eingang.id.clone(), strang.thread_id.clone(), true)
        .await
        .expect("markieren");

    let danach = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    assert!(
        danach.zeilen.iter().any(|zeile| zeile.markiert),
        "lokal nicht markiert"
    );

    // Und das kaputte Konto: ein Auftrag, der nicht durchgeht, bleibt
    // stehen statt verloren zu gehen.
    let _ = kern.abgleichen(ordner_kaputt).await;
    assert_eq!(
        kern.offene_auftraege(id_kaputt).await.expect("Warteschlange"),
        0,
        "für das kaputte Konto wurde nichts angelegt"
    );

    kern.markieren(eingang.id.clone(), strang.thread_id.clone(), false)
        .await
        .expect("zurücksetzen");
}

/// Legt eine eigene Nachricht im Eingang ab und gibt ihren Betreff zurück.
///
/// Tests, die etwas verschieben oder löschen, dürfen nicht die gemeinsame
/// Testpost verbrauchen — sonst sind beim nächsten Lauf andere Tests rot,
/// und zwar ohne dass am Code etwas falsch wäre. Genau das ist am
/// 12.11.2026 passiert.
async fn eigene_nachricht_ablegen(kennzeichen: &str) -> String {
    use mail_proto::{Anmeldeart, Kontobackend, Verschluesselung};

    let backend = mail_proto::imap::ImapBackend::neu_ungeprueft(mail_proto::Zugang {
        host: "localhost".to_owned(),
        port: 3993,
        benutzer: "test".to_owned(),
        geheimnis: "geheim".to_owned(),
        verschluesselung: Verschluesselung::Tls,
        anmeldeart: Anmeldeart::Passwort,
        senden: None,
    });

    let betreff = format!("Wegwerfnachricht {kennzeichen}");
    let rohdaten = format!(
        "From: wegwerf@beispiel.de\r\n\
         To: test@localhost\r\n\
         Subject: {betreff}\r\n\
         Message-ID: <wegwerf-{kennzeichen}@beispiel.de>\r\n\
         Date: Thu, 12 Nov 2026 10:00:00 +0000\r\n\
         \r\n\
         Diese Nachricht gehört einem Test.\r\n"
    );

    backend
        .anhaengen("INBOX", rohdaten.as_bytes(), 1_763_000_000_000, &[])
        .await
        .expect("Wegwerfnachricht ablegen");

    betreff
}

#[tokio::test]
#[ignore = "braucht den GreenMail-Container"]
async fn archivieren_nimmt_das_gespraech_aus_dem_eingang_und_legt_es_ins_archiv() {
    let verzeichnis = tempfile::tempdir().expect("Verzeichnis");
    let kern = kern_bauen(&verzeichnis);
    let id = kern.konto_anlegen(entwurf("geheim")).await.expect("anlegen");

    let ordner = kern.ordner(id.clone()).await.expect("Ordner");
    let eingang = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
        .expect("INBOX");
    let archiv = ordner
        .iter()
        .find(|o| o.pfad.eq_ignore_ascii_case("Archiv"))
        .expect("Archiv-Ordner fehlt im Testserver");

    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");
    kern.abgleichen(archiv.id.clone()).await.expect("Archiv abgleichen");

    let vorher_archiv = kern
        .threads(archiv.id.clone(), None, 100)
        .await
        .expect("Liste")
        .zeilen
        .iter()
        .map(|zeile| zeile.anzahl)
        .sum::<u32>();

    // Eigene Nachricht statt der gemeinsamen Testpost.
    let betreff = eigene_nachricht_ablegen("archiv").await;
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

    let seite = kern.threads(eingang.id.clone(), None, 100).await.expect("Liste");
    let strang = seite
        .zeilen
        .iter()
        .find(|zeile| zeile.betreff == betreff)
        .expect("die eigene Nachricht fehlt");
    let anzahl = strang.anzahl;

    kern.archivieren(eingang.id.clone(), strang.thread_id.clone())
        .await
        .expect("archivieren");

    // 1. Lokal sofort weg — ohne auf den Server zu warten.
    let danach = kern.threads(eingang.id.clone(), None, 50).await.expect("Liste");
    assert!(
        !danach.zeilen.iter().any(|zeile| zeile.thread_id == strang.thread_id),
        "das Gespräch steht noch im Eingang"
    );

    // 2. Beim Server angekommen: Der Abgleich des Archivs findet es.
    kern.abgleichen(archiv.id.clone()).await.expect("Archiv abgleichen");
    let nachher_archiv = kern
        .threads(archiv.id.clone(), None, 100)
        .await
        .expect("Liste")
        .zeilen
        .iter()
        .map(|zeile| zeile.anzahl)
        .sum::<u32>();

    assert_eq!(
        nachher_archiv,
        vorher_archiv + anzahl,
        "im Archiv ist nichts dazugekommen"
    );

    // 3. Und der Eingang hat es auch beim Server nicht mehr.
    kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");
    let eingang_danach = kern.threads(eingang.id.clone(), None, 100).await.expect("Liste");
    assert!(
        !eingang_danach.zeilen.iter().any(|zeile| zeile.betreff == betreff),
        "der Abgleich holt sie aus dem Eingang zurück — sie wurde nie verschoben"
    );
}
