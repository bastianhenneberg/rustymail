//! Eine Antwort bauen.
//!
//! Was eine Antwort zu einer Antwort macht, ist nicht das „Re:" im Betreff,
//! sondern `In-Reply-To` und `References`. Fehlen die, landet sie beim
//! Empfänger als neues Gespräch — und zwar auch dann, wenn der Betreff
//! stimmt. Das ist der häufigste Fehler in selbstgebauten Mailversendern und
//! fällt dem Absender nie auf, weil sein eigener Client die Zusammenführung
//! über den Betreff hinbekommt.

use mail_builder::MessageBuilder;

use mail_sync::threading;

pub struct Antwortentwurf<'a> {
    pub von_name: Option<&'a str>,
    pub von_adresse: &'a str,
    pub an: &'a str,
    pub betreff: &'a str,
    pub text: &'a str,
    /// Die Message-Id der Nachricht, auf die geantwortet wird.
    pub bezug: Option<&'a str>,
    /// Die bisherige Kette. Die eigene Kennung des Bezugs kommt ans Ende.
    pub references: &'a [String],
    pub gesendet_am: i64,
}

/// Baut die fertige RFC-5322-Nachricht und gibt sie samt ihrer Message-Id
/// zurück.
pub fn bauen(entwurf: &Antwortentwurf<'_>) -> (Vec<u8>, String) {
    let eigene_kennung = format!(
        "{}.{}@rustymail",
        entwurf.gesendet_am,
        // Kein Zufall: Dieselbe Eingabe muss dieselbe Nachricht ergeben,
        // sonst lässt sich ein Sendevorgang nicht wiederholen, ohne eine
        // zweite Mail zu erzeugen. Der Rumpf der Absenderadresse reicht zur
        // Unterscheidung, die Zeit erledigt den Rest.
        entwurf.von_adresse.replace(['@', '.'], "-")
    );

    let betreff = if threading::ist_antwort(entwurf.betreff) {
        entwurf.betreff.to_owned()
    } else {
        format!("Re: {}", entwurf.betreff)
    };

    let mut kette: Vec<String> = entwurf.references.to_vec();
    if let Some(bezug) = entwurf.bezug {
        let sauber = threading::saeubere_id(bezug);
        if !kette.iter().any(|eintrag| threading::saeubere_id(eintrag) == sauber) {
            kette.push(sauber);
        }
    }

    let mut bauer = MessageBuilder::new()
        .from((entwurf.von_name.unwrap_or(entwurf.von_adresse), entwurf.von_adresse))
        .to(entwurf.an)
        .subject(betreff)
        .message_id(eigene_kennung.clone())
        .date(entwurf.gesendet_am / 1000)
        .text_body(entwurf.text);

    if let Some(bezug) = entwurf.bezug {
        bauer = bauer.in_reply_to(threading::saeubere_id(bezug));
    }
    if !kette.is_empty() {
        bauer = bauer.references(kette);
    }

    (bauer.write_to_vec().unwrap_or_default(), eigene_kennung)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entwurf<'a>(betreff: &'a str, bezug: Option<&'a str>, references: &'a [String]) -> Antwortentwurf<'a> {
        Antwortentwurf {
            von_name: Some("Bastian"),
            von_adresse: "post@beispiel.de",
            an: "kunde@beispiel.de",
            betreff,
            text: "Mache ich.",
            bezug,
            references,
            gesendet_am: 1_700_000_000_000,
        }
    }

    fn als_text(rohdaten: &[u8]) -> String {
        String::from_utf8_lossy(rohdaten).to_string()
    }

    #[test]
    fn eine_antwort_traegt_bezug_und_kette() {
        let kette = vec!["erste@beispiel.de".to_owned()];
        let (rohdaten, _) = bauen(&entwurf("Angebot", Some("<zweite@beispiel.de>"), &kette));
        let text = als_text(&rohdaten);

        assert!(text.contains("In-Reply-To:"), "{text}");
        assert!(text.contains("zweite@beispiel.de"), "{text}");
        // Die Kette enthält BEIDE — die bisherige und den direkten Bezug.
        assert!(text.contains("References:"), "{text}");
        assert!(text.contains("erste@beispiel.de"), "{text}");
    }

    #[test]
    fn das_re_wird_nicht_gestapelt() {
        let (rohdaten, _) = bauen(&entwurf("Re: Angebot", None, &[]));
        let text = als_text(&rohdaten);
        assert!(text.contains("Subject: Re: Angebot"), "{text}");
        assert!(!text.contains("Re: Re:"), "Präfix gestapelt: {text}");
    }

    #[test]
    fn ohne_praefix_kommt_eines_dazu() {
        let (rohdaten, _) = bauen(&entwurf("Angebot", None, &[]));
        assert!(als_text(&rohdaten).contains("Subject: Re: Angebot"));
    }

    #[test]
    fn ein_deutsches_praefix_zaehlt_auch() {
        // „AW:" ist in deutschen Postfächern häufiger als „Re:". Wer nur auf
        // „Re:" prüft, baut „Re: AW: Angebot".
        let (rohdaten, _) = bauen(&entwurf("AW: Angebot", None, &[]));
        let text = als_text(&rohdaten);
        assert!(text.contains("Subject: AW: Angebot"), "{text}");
    }

    #[test]
    fn derselbe_entwurf_ergibt_dieselbe_kennung() {
        // Sonst erzeugt ein wiederholter Sendeversuch eine zweite Nachricht
        // statt derselben — und der Empfänger hat sie doppelt.
        let (_, erste) = bauen(&entwurf("Angebot", None, &[]));
        let (_, zweite) = bauen(&entwurf("Angebot", None, &[]));
        assert_eq!(erste, zweite);
    }

    #[test]
    fn der_bezug_steht_nicht_zweimal_in_der_kette() {
        let kette = vec!["<zweite@beispiel.de>".to_owned()];
        let (rohdaten, _) = bauen(&entwurf("Angebot", Some("zweite@beispiel.de"), &kette));
        let text = als_text(&rohdaten);
        let zeile = text
            .lines()
            .find(|zeile| zeile.starts_with("References:"))
            .unwrap_or_default();
        assert_eq!(zeile.matches("zweite@beispiel.de").count(), 1, "{zeile}");
    }
}
