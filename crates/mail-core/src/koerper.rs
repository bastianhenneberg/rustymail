//! MIME zerlegen: aus einer Rohfassung werden Text, HTML und Anhänge.
//!
//! Die Regel, die hier alles bestimmt: **Was nach aussen zeigt, zeigt nicht
//! nach aussen.** Eine Mail darf nichts nachladen, nichts ausführen und
//! nichts verraten. Ein Bild von einem fremden Server ist kein Bild, sondern
//! eine Rückmeldung an den Absender, dass gelesen wurde — mit Zeitpunkt und
//! Adresse. Deshalb wird HTML hier bereinigt, bevor es überhaupt in die
//! Datenbank geht.

use mail_parser::MessageParser;

use crate::ansicht::{Adresse, AnhangAnsicht, Koerper};

pub struct Zerlegung {
    pub text: Option<String>,
    pub html: Option<String>,
    pub anhaenge: Vec<AnhangAnsicht>,
    pub an: Vec<Adresse>,
    pub kopie: Vec<Adresse>,
}

pub fn zerlegen(rohdaten: &[u8], nachricht_id: &str) -> Option<Zerlegung> {
    let nachricht = MessageParser::default().parse(rohdaten)?;

    // NICHT `text_bodies()`/`html_bodies()` nehmen: Die rechnen ineinander um
    // — „text parts converted" steht so im Quelltext von mail-parser. Eine
    // reine Textmail liefert dort ein ERZEUGTES HTML, und wer das für echtes
    // HTML hält, zeigt am Ende den Text ohne seine Absätze. Genau das war am
    // 12.11.2026 im Fenster zu sehen.
    //
    // Hier wird deshalb der Teil selbst angesehen: Nur ein wirkliches
    // `text/html` gilt als HTML, nur ein wirkliches `text/plain` als Text.
    let html = nachricht
        .html_body()
        .next()
        .filter(|teil| ist_untertyp(teil, "html"))
        .and_then(|teil| teil.text())
        .map(|inhalt| bereinigen(&inhalt));

    let text = nachricht
        .text_body()
        .next()
        .filter(|teil| ist_untertyp(teil, "plain"))
        .and_then(|teil| teil.text())
        .map(|inhalt| inhalt.to_string());

    let mut anhaenge = Vec::new();
    for (stelle, teil) in nachricht.attachments().enumerate() {
        let dateiname = teil
            .attachment_name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Anhang-{}", stelle + 1));

        let medientyp = teil
            .content_type()
            .map(|typ| match typ.subtype() {
                Some(untertyp) => format!("{}/{}", typ.ctype(), untertyp),
                None => typ.ctype().to_owned(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_owned());

        // Ein Teil mit Content-Id gehört in den Text, nicht in die Liste
        // darunter — sonst steht unter jeder Signatur mit Logo ein
        // „1 Anhang", den niemand geschickt hat.
        let content_id = teil.content_id().map(|kennung| {
            kennung.trim().trim_start_matches('<').trim_end_matches('>').to_owned()
        });

        anhaenge.push(AnhangAnsicht {
            id: format!("{nachricht_id}/anhang/{stelle}"),
            dateiname,
            medientyp,
            groesse: teil.decoded_len() as u64,
            inline: teil.is_inline() || content_id.is_some(),
            content_id,
        });
    }

    Some(Zerlegung {
        text,
        html,
        anhaenge,
        an: adressen(nachricht.to()),
        kopie: adressen(nachricht.cc()),
    })
}

fn ist_untertyp(teil: &mail_parser::MessagePart<'_>, untertyp: &str) -> bool {
    teil.content_type()
        .and_then(|typ| typ.subtype())
        .is_some_and(|wert| wert.eq_ignore_ascii_case(untertyp))
}

fn adressen(liste: Option<mail_parser::AddressList<'_>>) -> Vec<Adresse> {
    let Some(liste) = liste else {
        return Vec::new();
    };

    // `mailboxes()` statt `iter()`: Eine Adressliste darf Gruppen enthalten
    // („Vertrieb: a@x, b@x;"), und `iter()` liefert die Gruppe als EINEN
    // Eintrag ohne Adresse. Die Empfänger darin fielen damit still weg.
    liste
        .mailboxes()
        .filter_map(|eintrag| {
            eintrag.address().map(|adresse| Adresse {
                name: eintrag.name().map(str::to_owned),
                adresse: adresse.to_owned(),
            })
        })
        .collect()
}

/// Was aus dem HTML entfernt wird, bevor es irgendwo landet.
///
/// Dies ist **kein** vollständiger Bereiniger — ein solcher gehört mit dem
/// Zeichner zusammen (#9869) und braucht einen HTML-Parser statt
/// Zeichenkettensuche. Was hier passiert, ist der Riegel davor: Die
/// gefährlichsten Elemente verschwinden schon beim Zerlegen, damit sie gar
/// nicht erst in die Datenbank kommen. Was in der Datenbank liegt, taucht
/// sonst in einem Jahr in einer Suche, einem Export oder einem Vorschautext
/// wieder auf — an einer Stelle, an die bei der Anzeige niemand gedacht hat.
fn bereinigen(html: &str) -> String {
    let mut sauber = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = finde_ohne_gross(rest, '<') {
        sauber.push_str(&rest[..start]);
        rest = &rest[start..];

        let mut geschnitten = false;
        for element in ["script", "iframe", "object", "embed", "link", "meta", "base"] {
            if beginnt_mit_element(rest, element) {
                rest = ueberspringe_element(rest, element);
                geschnitten = true;
                break;
            }
        }
        if geschnitten {
            continue;
        }

        // Ein einzelnes `<` ohne Element ist Text und darf stehen bleiben.
        let ende = rest.find('>').map(|stelle| stelle + 1).unwrap_or(rest.len());
        let marke = &rest[..ende];
        sauber.push_str(&ereignisse_entfernen(marke));
        rest = &rest[ende..];
    }
    sauber.push_str(rest);
    sauber
}

fn finde_ohne_gross(text: &str, zeichen: char) -> Option<usize> {
    text.find(zeichen)
}

fn beginnt_mit_element(rest: &str, element: &str) -> bool {
    let klein = rest.to_lowercase();
    klein.starts_with(&format!("<{element}"))
        && klein[element.len() + 1..]
            .chars()
            .next()
            .is_none_or(|zeichen| zeichen.is_whitespace() || zeichen == '>' || zeichen == '/')
}

fn ueberspringe_element<'a>(rest: &'a str, element: &str) -> &'a str {
    let klein = rest.to_lowercase();
    let schluss = format!("</{element}>");
    match klein.find(&schluss) {
        Some(stelle) => &rest[stelle + schluss.len()..],
        // Kein Schlusszeichen: Dann gehört der Rest zum Element. Lieber zu
        // viel wegwerfen als ein offenes `<script>` stehen lassen.
        None => match rest.find('>') {
            Some(stelle) if !klein.starts_with("<script") => &rest[stelle + 1..],
            _ => "",
        },
    }
}

/// `onclick=…`, `onerror=…` und `javascript:` aus einer Marke entfernen.
fn ereignisse_entfernen(marke: &str) -> String {
    let klein = marke.to_lowercase();
    if !klein.contains(" on") && !klein.contains("javascript:") {
        return marke.to_owned();
    }

    let mut sauber = String::with_capacity(marke.len());
    let zeichen: Vec<char> = marke.chars().collect();
    let kleinbuchstaben: Vec<char> = klein.chars().collect();
    let mut stelle = 0;

    while stelle < zeichen.len() {
        let beginnt_ereignis = kleinbuchstaben[stelle].is_whitespace()
            && kleinbuchstaben
                .get(stelle + 1..stelle + 3)
                .is_some_and(|paar| paar == ['o', 'n']);

        if beginnt_ereignis {
            // Bis zum Ende des Attributwerts springen.
            let mut lauf = stelle + 1;
            while lauf < zeichen.len() && zeichen[lauf] != '=' && zeichen[lauf] != '>' {
                lauf += 1;
            }
            if lauf < zeichen.len() && zeichen[lauf] == '=' {
                lauf += 1;
                while lauf < zeichen.len() && zeichen[lauf].is_whitespace() {
                    lauf += 1;
                }
                let anfuehrung = zeichen.get(lauf).copied();
                match anfuehrung {
                    Some(zeichen_auf @ ('"' | '\'')) => {
                        lauf += 1;
                        while lauf < zeichen.len() && zeichen[lauf] != zeichen_auf {
                            lauf += 1;
                        }
                        lauf += 1;
                    }
                    _ => {
                        while lauf < zeichen.len()
                            && !zeichen[lauf].is_whitespace()
                            && zeichen[lauf] != '>'
                        {
                            lauf += 1;
                        }
                    }
                }
            }
            stelle = lauf;
            continue;
        }

        sauber.push(zeichen[stelle]);
        stelle += 1;
    }

    sauber.replace("javascript:", "entschaerft:")
}

/// Aus HTML einen lesbaren Text gewinnen.
///
/// Notnagel für Nachrichten, die **nur** HTML mitbringen — und das sind
/// mehr, als man denkt: Fast jeder Newsletter und jede Software, die Mails
/// aus Vorlagen baut, lässt den Textteil weg. Ohne diesen Weg bliebe die
/// Anzeige dort leer.
pub fn text_aus_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 2);
    let mut in_marke = false;
    let mut letzter_war_leer = true;

    for zeichen in html.chars() {
        match zeichen {
            '<' => in_marke = true,
            '>' => {
                in_marke = false;
                // Eine Marke trennt Wörter — ohne das klebt „EndeAnfang"
                // zusammen.
                if !letzter_war_leer {
                    text.push(' ');
                    letzter_war_leer = true;
                }
            }
            _ if in_marke => {}
            zeichen if zeichen.is_whitespace() => {
                if !letzter_war_leer {
                    text.push(' ');
                    letzter_war_leer = true;
                }
            }
            zeichen => {
                text.push(zeichen);
                letzter_war_leer = false;
            }
        }
    }

    entitaeten(text.trim())
}

fn entitaeten(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Baut den Körper für die Anzeige.
pub fn als_koerper(text: Option<String>, html: Option<String>) -> Koerper {
    match (text, html) {
        // Liegt beides vor (multipart/alternative), gewinnt das HTML: Der
        // Textteil ist dort oft eine lieblose Kurzfassung. Der Ersatztext
        // wird trotzdem aus dem HTML gewonnen und nicht aus dem Textteil —
        // sonst zeigen zwei Oberflächen zwei verschiedene Inhalte.
        (_, Some(html)) => {
            let dokument = crate::dokument::aus_html(&html);
            // Der Ersatztext kommt aus dem DOKUMENT, nicht aus dem HTML:
            // Sonst zeigen Vorschau und geöffnete Mail zwei verschiedene
            // Inhalte, und niemand weiss, welcher stimmt.
            let text_ersatz = dokument.als_text();
            Koerper::Html { dokument, text_ersatz }
        }
        (Some(text), None) => Koerper::Text { inhalt: text },
        (None, None) => Koerper::Text { inhalt: String::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skripte_verschwinden_samt_inhalt() {
        let roh = r#"<p>Hallo</p><script>alert('x')</script><p>Welt</p>"#;
        let sauber = bereinigen(roh);
        assert!(!sauber.to_lowercase().contains("script"), "{sauber}");
        assert!(!sauber.contains("alert"), "der Inhalt blieb stehen: {sauber}");
        assert!(sauber.contains("Hallo") && sauber.contains("Welt"));
    }

    #[test]
    fn ein_unabgeschlossenes_skript_frisst_den_rest() {
        // Ohne Schlusszeichen ist nicht entscheidbar, wo das Skript endet.
        // Dann lieber den Rest wegwerfen, als ein offenes <script> stehen
        // zu lassen.
        let sauber = bereinigen("<p>Hallo</p><script>alert('x')");
        assert!(!sauber.contains("alert"), "{sauber}");
        assert!(sauber.contains("Hallo"));
    }

    #[test]
    fn rahmen_und_fremde_einbindungen_fliegen_raus() {
        for boese in [
            "<iframe src='https://fremd.de/zaehler'></iframe>",
            "<object data='x.swf'></object>",
            "<embed src='x'>",
            "<link rel='stylesheet' href='https://fremd.de/a.css'>",
            "<base href='https://fremd.de/'>",
            "<meta http-equiv='refresh' content='0;url=https://fremd.de'>",
        ] {
            let sauber = bereinigen(&format!("<p>Text</p>{boese}"));
            assert!(
                !sauber.to_lowercase().contains("fremd.de") && !sauber.contains("swf"),
                "blieb stehen: {boese} → {sauber}"
            );
            assert!(sauber.contains("Text"));
        }
    }

    #[test]
    fn ereignis_attribute_werden_entfernt() {
        let sauber = bereinigen(r#"<img src="bild.png" onerror="stehlen()" alt="x">"#);
        assert!(!sauber.to_lowercase().contains("onerror"), "{sauber}");
        assert!(!sauber.contains("stehlen"), "{sauber}");
        // Der Rest der Marke bleibt — das Bild selbst ist nicht das Problem.
        assert!(sauber.contains("bild.png") && sauber.contains("alt="), "{sauber}");
    }

    #[test]
    fn javascript_adressen_werden_entschaerft() {
        let sauber = bereinigen(r#"<a href="javascript:boese()">klick</a>"#);
        assert!(!sauber.contains("javascript:"), "{sauber}");
        assert!(sauber.contains("klick"));
    }

    #[test]
    fn gewoehnliches_html_bleibt_unangetastet() {
        // Der wichtigere Prüfpunkt: Ein Bereiniger, der zu viel wegnimmt,
        // macht Newsletter unlesbar — und das fällt niemandem als
        // Sicherheitsmassnahme auf, sondern als kaputte Anzeige.
        let roh = r#"<table width="100%"><tr><td style="color:#333">Angebot <b>1147</b></td></tr></table>"#;
        assert_eq!(bereinigen(roh), roh);
    }

    #[test]
    fn aus_html_wird_lesbarer_text() {
        let text = text_aus_html("<p>Guten Tag,</p><p>anbei das <b>Angebot</b>.</p>");
        assert_eq!(text, "Guten Tag, anbei das Angebot .");
    }

    #[test]
    fn entitaeten_werden_aufgeloest() {
        assert_eq!(text_aus_html("<p>M&uuml;ller &amp; S&ouml;hne</p>").contains("&amp;"), false);
        assert!(text_aus_html("<p>a &lt; b</p>").contains("a < b"));
    }

    #[test]
    fn eine_reine_textmail_bleibt_text_und_behaelt_ihre_absaetze() {
        // Der Fehler, der im Fenster sichtbar war: mail-parser rechnet
        // Textteile in HTML um, wenn man `html_bodies()` fragt. Wer das für
        // echtes HTML hält, macht daraus wieder Text — und dabei werden aus
        // Absätzen Leerzeichen.
        let roh = b"From: a@x\r\nTo: b@x\r\nSubject: Test\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nGuten Tag,\r\n\r\nkoennen Sie das anpassen?\r\n\r\nViele Gruesse\r\n";
        let zerlegung = zerlegen(roh, "n1").expect("zerlegbar");

        assert!(zerlegung.html.is_none(), "erzeugtes HTML als echtes genommen");
        let text = zerlegung.text.expect("Textteil");
        assert!(
            text.contains("\n\n") || text.matches('\n').count() >= 2,
            "die Absätze sind weg: {text:?}"
        );

        match als_koerper(Some(text), None) {
            Koerper::Text { inhalt } => assert!(inhalt.contains("Guten Tag")),
            andere => panic!("falscher Körper: {andere:?}"),
        }
    }

    #[test]
    fn echtes_html_wird_als_html_erkannt() {
        let roh = b"From: a@x\r\nTo: b@x\r\nSubject: Test\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hallo <b>Welt</b></p>\r\n";
        let zerlegung = zerlegen(roh, "n1").expect("zerlegbar");
        assert!(zerlegung.html.is_some(), "echtes HTML nicht erkannt");
        assert!(zerlegung.html.unwrap().contains("<b>"));
    }

    #[test]
    fn ohne_textteil_traegt_das_html_den_text() {
        let koerper = als_koerper(None, Some("<p>Nur HTML</p>".to_owned()));
        match koerper {
            Koerper::Html { text_ersatz, .. } => assert!(text_ersatz.contains("Nur HTML")),
            andere => panic!("falscher Körper: {andere:?}"),
        }
    }
}
