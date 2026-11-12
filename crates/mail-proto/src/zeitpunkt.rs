//! Unix-Millisekunden als IMAP-Zeitpunkt.
//!
//! IMAP verlangt für `APPEND` ein Datum in der Form
//! `"17-Jul-1996 02:44:25 -0700"` — mit Anführungszeichen, englischem
//! Monatskürzel und Zonenversatz. Ohne diese Angabe setzt der Server den
//! Zeitpunkt des Einlegens, und eine verschobene oder kopierte Nachricht
//! trägt dann das Datum des Umzugstags. In einem nach Datum sortierten
//! Postfach rutscht sie damit nach oben und wirkt wie neu.
//!
//! Gerechnet wird ohne Fremdbibliothek: Howard Hinnants Verfahren zur
//! Umrechnung von Tagen seit 1970 in ein bürgerliches Datum. Der Zonenversatz
//! ist immer `+0000` — der Zeitpunkt stimmt damit, und eine falsche Zone wäre
//! schlimmer als eine ungewohnte.

const MONATE: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn als_imap_datum(millisekunden: i64) -> String {
    let sekunden = millisekunden.div_euclid(1000);
    let tage = sekunden.div_euclid(86_400);
    let rest = sekunden.rem_euclid(86_400);

    let (jahr, monat, tag) = buergerliches_datum(tage);
    let stunde = rest / 3600;
    let minute = (rest % 3600) / 60;
    let sekunde = rest % 60;

    format!(
        "\"{tag:02}-{}-{jahr:04} {stunde:02}:{minute:02}:{sekunde:02} +0000\"",
        MONATE[(monat - 1) as usize]
    )
}

/// Tage seit dem 1.1.1970 → (Jahr, Monat, Tag).
///
/// Nach Howard Hinnant: Das Jahr beginnt im März, dadurch fällt der
/// Schalttag ans Ende und die Fallunterscheidung entfällt. Die
/// Verschiebung 719_468 setzt den Nullpunkt vom 1.3.0000 auf den 1.1.1970.
fn buergerliches_datum(tage_seit_1970: i64) -> (i64, i64, i64) {
    let tage = tage_seit_1970 + 719_468;
    let aera = if tage >= 0 { tage } else { tage - 146_096 } / 146_097;
    let tag_der_aera = tage - aera * 146_097;
    let jahr_der_aera =
        (tag_der_aera - tag_der_aera / 1460 + tag_der_aera / 36_524 - tag_der_aera / 146_096)
            / 365;
    let jahr = jahr_der_aera + aera * 400;
    let tag_des_jahres =
        tag_der_aera - (365 * jahr_der_aera + jahr_der_aera / 4 - jahr_der_aera / 100);
    let verschobener_monat = (5 * tag_des_jahres + 2) / 153;
    let tag = tag_des_jahres - (153 * verschobener_monat + 2) / 5 + 1;
    let monat = if verschobener_monat < 10 {
        verschobener_monat + 3
    } else {
        verschobener_monat - 9
    };

    (if monat <= 2 { jahr + 1 } else { jahr }, monat, tag)
}

/// `2026-11-12T14:07:33Z` → Unix-Millisekunden.
///
/// JMAP gibt Zeitpunkte nach RFC 3339 heraus. Eine Datumsbibliothek dafür
/// einzubinden wäre viel für ein festes Format — die Umkehrung der Rechnung
/// oben genügt. Was nicht passt, gibt `None`: Ein geratener Zeitpunkt wäre
/// schlimmer als keiner, denn danach wird sortiert.
pub fn aus_rfc3339(text: &str) -> Option<i64> {
    let text = text.trim();
    let (datum, rest) = text.split_once('T').or_else(|| text.split_once(' '))?;

    let mut datumsteile = datum.split('-');
    let jahr: i64 = datumsteile.next()?.parse().ok()?;
    let monat: i64 = datumsteile.next()?.parse().ok()?;
    let tag: i64 = datumsteile.next()?.parse().ok()?;

    // Zonenangabe abtrennen. `Z` heisst UTC; ein Versatz wird verrechnet.
    let (zeit, versatz_sekunden) = if let Some(ohne) = rest.strip_suffix('Z') {
        (ohne, 0)
    } else if let Some(stelle) = rest.rfind(['+', '-']) {
        let (zeit, versatz) = rest.split_at(stelle);
        let vorzeichen = if versatz.starts_with('-') { -1 } else { 1 };
        let versatz = &versatz[1..];
        let (stunden, minuten) = versatz.split_once(':').unwrap_or((versatz, "0"));
        let stunden: i64 = stunden.parse().ok()?;
        let minuten: i64 = minuten.parse().unwrap_or(0);
        (zeit, vorzeichen * (stunden * 3600 + minuten * 60))
    } else {
        (rest, 0)
    };

    let mut zeitteile = zeit.split(':');
    let stunde: i64 = zeitteile.next()?.parse().ok()?;
    let minute: i64 = zeitteile.next()?.parse().ok()?;
    let sekunde: f64 = zeitteile.next().unwrap_or("0").parse().unwrap_or(0.0);

    // Bereiche prüfen, bevor gerechnet wird. Die Tagesrechnung normalisiert
    // sonst klaglos: Aus „Monat 13, Tag 45, Stunde 99" würde ein gültiger
    // Zeitpunkt irgendwo im Folgejahr — und eine Liste, die danach sortiert,
    // zeigt die Nachricht an einer Stelle, an der sie niemand sucht.
    if !(1..=12).contains(&monat)
        || !(1..=31).contains(&tag)
        || !(0..=23).contains(&stunde)
        || !(0..=59).contains(&minute)
        // 60 ist erlaubt: Schaltsekunde.
        || !(0.0..=60.0).contains(&sekunde)
    {
        return None;
    }

    let tage = tage_seit_1970(jahr, monat, tag);
    let sekunden =
        tage * 86_400 + stunde * 3600 + minute * 60 + sekunde as i64 - versatz_sekunden;
    Some(sekunden * 1000)
}

/// Umkehrung von `buergerliches_datum`.
fn tage_seit_1970(jahr: i64, monat: i64, tag: i64) -> i64 {
    let jahr = if monat <= 2 { jahr - 1 } else { jahr };
    let aera = if jahr >= 0 { jahr } else { jahr - 399 } / 400;
    let jahr_der_aera = jahr - aera * 400;
    let verschobener_monat = if monat > 2 { monat - 3 } else { monat + 9 };
    let tag_des_jahres = (153 * verschobener_monat + 2) / 5 + tag - 1;
    let tag_der_aera =
        jahr_der_aera * 365 + jahr_der_aera / 4 - jahr_der_aera / 100 + tag_des_jahres;
    aera * 146_097 + tag_der_aera - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bekannte_zeitpunkte() {
        // Der Nullpunkt.
        assert_eq!(als_imap_datum(0), "\"01-Jan-1970 00:00:00 +0000\"");
        // Ein Schalttag — der Fall, an dem selbst gebaute Datumsrechnungen
        // reihenweise scheitern.
        assert_eq!(
            als_imap_datum(1_709_164_800_000),
            "\"29-Feb-2024 00:00:00 +0000\""
        );
        // Jahreswechsel, eine Sekunde davor.
        assert_eq!(
            als_imap_datum(1_735_689_599_000),
            "\"31-Dec-2024 23:59:59 +0000\""
        );
        // Ein gewöhnlicher Zeitpunkt mit Uhrzeit.
        assert_eq!(
            als_imap_datum(1_763_000_000_000),
            "\"13-Nov-2025 02:13:20 +0000\""
        );
    }

    #[test]
    fn vor_1970_bricht_nicht() {
        // Kommt vor: Nachrichten mit falsch gestellter Uhr tragen
        // Zeitstempel aus den Sechzigern. Eine Division, die dabei in die
        // falsche Richtung rundet, erzeugt den 32. eines Monats — und der
        // Server lehnt den ganzen APPEND ab.
        let datum = als_imap_datum(-86_400_000);
        assert_eq!(datum, "\"31-Dec-1969 00:00:00 +0000\"");
    }

    #[test]
    fn rfc3339_wird_gelesen() {
        assert_eq!(aus_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(aus_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        assert_eq!(aus_rfc3339("2025-11-13T02:13:20Z"), Some(1_763_000_000_000));
        // Mit Zonenversatz: 12:00 in +02:00 ist 10:00 UTC.
        assert_eq!(
            aus_rfc3339("2025-11-13T12:00:00+02:00"),
            aus_rfc3339("2025-11-13T10:00:00Z")
        );
        // Bruchteile von Sekunden kommen vor und dürfen nicht stören.
        assert_eq!(
            aus_rfc3339("2025-11-13T02:13:20.123Z"),
            Some(1_763_000_000_000)
        );
    }

    #[test]
    fn unlesbares_gibt_nichts_zurueck() {
        // Lieber kein Zeitpunkt als ein geratener — danach wird sortiert.
        for murks in ["", "heute", "2026-13-45T99:99:99Z", "2026-11-12"] {
            assert_eq!(aus_rfc3339(murks), None, "bei {murks:?}");
        }
    }

    #[test]
    fn hin_und_zurueck_bleibt_gleich() {
        for millisekunden in [0, 1_700_000_000_000, 1_763_000_000_000, -86_400_000] {
            let imap = als_imap_datum(millisekunden);
            // Aus dem IMAP-Format wieder einen Zeitpunkt zu machen ist nicht
            // Aufgabe dieser Kiste — geprüft wird die RFC-3339-Richtung
            // gegen dieselbe Tagesrechnung.
            assert!(imap.len() > 10);
        }
        assert_eq!(tage_seit_1970(1970, 1, 1), 0);
        assert_eq!(tage_seit_1970(2024, 2, 29), 1_709_164_800 / 86_400);
    }

    #[test]
    fn die_form_passt_zur_vorschrift() {
        let datum = als_imap_datum(1_700_000_000_000);
        assert!(datum.starts_with('"') && datum.ends_with('"'), "{datum}");
        let innen = datum.trim_matches('"');
        assert_eq!(innen.len(), 26, "Länge laut RFC 3501: {innen}");
        assert!(innen.ends_with(" +0000"));
    }
}
