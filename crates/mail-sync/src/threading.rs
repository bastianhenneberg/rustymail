//! Nachrichten zu Gesprächen zusammenfassen.
//!
//! Kein vollständiger JWZ-Baum: Eine Gesprächsliste braucht **Gruppen**,
//! keine Hierarchie. Wer den Baum baut, um ihn dann flachzudrücken, hat
//! Arbeit gemacht, die niemand sieht.
//!
//! Zwei Wege führen zusammen:
//!
//! 1. **References und In-Reply-To.** Der verlässliche Weg. Jede Kennung in
//!    diesen Kopfzeilen, die wir schon kennen, zieht die Nachricht in deren
//!    Gespräch.
//! 2. **Der Betreff — aber nur mit Antwortpräfix.** Manche Absender
//!    (Formular-Postfächer, alte Branchensoftware, manche Telefon-Clients)
//!    setzen gar keine References. Ohne Rückfall landet dann jede Antwort als
//!    eigenes Gespräch.
//!
//! Der Betreff-Rückfall ist absichtlich eng: **nur wenn der Betreff ein
//! `Re:`/`AW:`/`WG:`/`Fwd:` trägt.** Sonst würde jede Nachricht mit dem
//! Betreff „Rechnung" in einem einzigen Riesengespräch landen — und zwei
//! Vorgänge zu verschmelzen ist schlimmer, als einen zu zerreissen: Das
//! Zerrissene sieht man, das Verschmolzene führt dazu, dass jemand die
//! falsche Mail beantwortet.

/// Was die Zuordnung ergeben hat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zuordnung {
    /// Der Schlüssel, unter dem die Nachricht künftig liegt.
    pub schluessel: String,
    /// Gespräche, die durch diese Nachricht zu einem werden. Das passiert,
    /// wenn eine Antwort zwei bisher getrennte Stränge referenziert — und es
    /// passiert wirklich, etwa wenn jemand zwei Mails zusammen beantwortet.
    pub zusammenlegen: Vec<String>,
}

/// Woher die bekannten Zuordnungen kommen. Als Merkmal, damit die Rechnung
/// ohne Datenbank prüfbar bleibt.
pub trait BekannteGespraeche {
    /// Der Gesprächsschlüssel einer bekannten Message-Id.
    fn nach_message_id(&self, message_id: &str) -> Option<String>;
    /// Der Gesprächsschlüssel zu einem normalisierten Betreff — nur für den
    /// Rückfall, und nur innerhalb eines sinnvollen Zeitfensters. Was
    /// „sinnvoll" heisst, entscheidet die Umsetzung; vier Wochen sind ein
    /// brauchbarer Wert.
    fn nach_betreff(&self, normalisierter_betreff: &str) -> Option<String>;
}

/// Eine Nachricht, wie sie in die Zuordnung geht.
#[derive(Debug, Clone)]
pub struct Eingang {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub betreff: String,
    pub gesendet_am: i64,
}

/// Das Ergebnis für einen ganzen Stapel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gruppierung {
    /// Je Eingang, in derselben Reihenfolge, der Gesprächsschlüssel.
    pub schluessel: Vec<String>,
    /// Bestehende Gespräche, die zusammenwandern: (von, nach).
    pub zusammenlegen: Vec<(String, String)>,
}

/// Ordnet einen ganzen Stapel auf einmal zu — **reihenfolgenunabhängig**.
///
/// Das ist der Punkt, an dem der naive Weg bricht. Verarbeitet man Nachricht
/// für Nachricht, muss die Ausgangsmail vor ihrer Antwort kommen, sonst
/// eröffnet die Antwort ein eigenes Gespräch und die Ausgangsmail danach ein
/// zweites. „Dann sortiere nach Datum" klingt wie die Lösung und ist keine:
/// Zwei Mails derselben Sekunde haben keine verlässliche Ordnung, Absender
/// mit falsch gestellter Uhr sind Alltag, und beim Nachladen älterer Seiten
/// kommt die Antwort ohnehin zuerst.
///
/// Deshalb: zuerst jede Nachricht in ihre eigene Menge, dann jede bekannte
/// Referenz als Verbindung zwischen zwei Mengen, und am Ende trägt jede
/// Menge einen Schlüssel. Wer schon in der Datenbank steht, gibt seinen
/// Schlüssel vor — ein bestehendes Gespräch soll nicht umbenannt werden,
/// nur weil eine Antwort dazukommt.
pub fn gruppieren(stapel: &[Eingang], bekannt: &impl BekannteGespraeche) -> Gruppierung {
    use std::collections::{HashMap, HashSet};

    // Minimale Union-Find-Struktur über Zeichenketten.
    struct Mengen {
        eltern: HashMap<String, String>,
    }

    impl Mengen {
        fn anlegen(&mut self, schluessel: &str) {
            self.eltern
                .entry(schluessel.to_owned())
                .or_insert_with(|| schluessel.to_owned());
        }

        fn wurzel(&mut self, schluessel: &str) -> String {
            let mut aktuell = schluessel.to_owned();
            loop {
                let elter = match self.eltern.get(&aktuell) {
                    Some(elter) => elter.clone(),
                    None => {
                        self.anlegen(&aktuell);
                        return aktuell;
                    }
                };
                if elter == aktuell {
                    return aktuell;
                }
                aktuell = elter;
            }
        }

        fn verbinden(&mut self, links: &str, rechts: &str) {
            let (a, b) = (self.wurzel(links), self.wurzel(rechts));
            if a != b {
                // Kleinere Zeichenkette wird Wurzel — allein der
                // Bestimmtheit wegen: Derselbe Stapel muss zweimal dasselbe
                // ergeben, sonst wandern Gespräche bei jedem Abgleich.
                let (gewinner, verlierer) = if a <= b { (a, b) } else { (b, a) };
                self.eltern.insert(verlierer, gewinner);
            }
        }
    }

    let mut mengen = Mengen { eltern: HashMap::new() };
    let mut aus_der_datenbank: HashSet<String> = HashSet::new();

    // Schritt 1: eigener Schlüssel je Nachricht.
    let eigene: Vec<String> = stapel
        .iter()
        .map(|eingang| {
            let normalisiert = normalisiere_betreff(&eingang.betreff);
            match eingang.message_id.as_deref() {
                Some(kennung) if !kennung.trim().is_empty() => saeubere_id(kennung),
                _ => format!("betreff:{normalisiert}"),
            }
        })
        .collect();

    for schluessel in &eigene {
        mengen.anlegen(schluessel);
    }

    // Wer im Stapel welche Kennung trägt.
    let mut kennungen: HashMap<String, usize> = HashMap::new();
    for (stelle, eingang) in stapel.iter().enumerate() {
        if let Some(kennung) = eingang.message_id.as_deref() {
            kennungen.insert(saeubere_id(kennung), stelle);
        }
    }

    // Schritt 2: Referenzen verbinden.
    for (stelle, eingang) in stapel.iter().enumerate() {
        let mut kandidaten: Vec<String> = Vec::new();
        if let Some(eltern) = eingang.in_reply_to.as_deref() {
            kandidaten.push(saeubere_id(eltern));
        }
        kandidaten.extend(eingang.references.iter().rev().map(|wert| saeubere_id(wert)));

        let mut verbunden = false;
        for kandidat in &kandidaten {
            if let Some(&andere) = kennungen.get(kandidat) {
                if andere != stelle {
                    mengen.verbinden(&eigene[stelle], &eigene[andere]);
                    verbunden = true;
                }
            } else if let Some(bestehend) = bekannt.nach_message_id(kandidat) {
                mengen.anlegen(&bestehend);
                aus_der_datenbank.insert(bestehend.clone());
                mengen.verbinden(&eigene[stelle], &bestehend);
                verbunden = true;
            }
        }

        // Schritt 3: der enge Betreff-Rückfall — nur ohne Referenztreffer
        // und nur bei einem Antwortpräfix.
        if !verbunden && ist_antwort(&eingang.betreff) {
            let normalisiert = normalisiere_betreff(&eingang.betreff);
            if normalisiert.is_empty() {
                continue;
            }

            let im_stapel = stapel.iter().enumerate().find(|(andere, kandidat)| {
                *andere != stelle && normalisiere_betreff(&kandidat.betreff) == normalisiert
            });

            if let Some((andere, _)) = im_stapel {
                mengen.verbinden(&eigene[stelle], &eigene[andere]);
            } else if let Some(bestehend) = bekannt.nach_betreff(&normalisiert) {
                mengen.anlegen(&bestehend);
                aus_der_datenbank.insert(bestehend.clone());
                mengen.verbinden(&eigene[stelle], &bestehend);
            }
        }
    }

    // Schritt 4: je Menge einen Namen bestimmen. Ein Schlüssel aus der
    // Datenbank gewinnt — ein bestehendes Gespräch behält seinen Namen, auch
    // wenn eine ältere Nachricht nachträglich dazukommt.
    let mut name_der_menge: HashMap<String, String> = HashMap::new();
    for bestehend in &aus_der_datenbank {
        let wurzel = mengen.wurzel(bestehend);
        name_der_menge
            .entry(wurzel)
            .and_modify(|vorhanden| {
                if bestehend < vorhanden {
                    *vorhanden = bestehend.clone();
                }
            })
            .or_insert_with(|| bestehend.clone());
    }

    // Ohne Datenbank-Treffer: die Nachricht mit dem frühesten Zeitpunkt gibt
    // den Namen. Das ist die Wurzel des Strangs, und sie bleibt dieselbe,
    // wenn später Antworten dazukommen.
    let mut reihenfolge: Vec<usize> = (0..stapel.len()).collect();
    reihenfolge.sort_by(|links, rechts| {
        stapel[*links]
            .gesendet_am
            .cmp(&stapel[*rechts].gesendet_am)
            .then_with(|| eigene[*links].cmp(&eigene[*rechts]))
    });
    for stelle in reihenfolge {
        let wurzel = mengen.wurzel(&eigene[stelle]);
        name_der_menge
            .entry(wurzel)
            .or_insert_with(|| eigene[stelle].clone());
    }

    let schluessel: Vec<String> = (0..stapel.len())
        .map(|stelle| {
            let wurzel = mengen.wurzel(&eigene[stelle]);
            name_der_menge
                .get(&wurzel)
                .cloned()
                .unwrap_or_else(|| eigene[stelle].clone())
        })
        .collect();

    let mut zusammenlegen: Vec<(String, String)> = aus_der_datenbank
        .iter()
        .filter_map(|bestehend| {
            let wurzel = mengen.wurzel(bestehend);
            let name = name_der_menge.get(&wurzel)?;
            (name != bestehend).then(|| (bestehend.clone(), name.clone()))
        })
        .collect();
    zusammenlegen.sort();

    Gruppierung { schluessel, zusammenlegen }
}

pub fn zuordnen(
    message_id: Option<&str>,
    in_reply_to: Option<&str>,
    references: &[String],
    betreff: &str,
    bekannt: &impl BekannteGespraeche,
) -> Zuordnung {
    let stapel = [Eingang {
        message_id: message_id.map(str::to_owned),
        in_reply_to: in_reply_to.map(str::to_owned),
        references: references.to_vec(),
        betreff: betreff.to_owned(),
        gesendet_am: 0,
    }];
    let gruppierung = gruppieren(&stapel, bekannt);

    Zuordnung {
        schluessel: gruppierung.schluessel[0].clone(),
        zusammenlegen: gruppierung
            .zusammenlegen
            .into_iter()
            .map(|(von, _nach)| von)
            .collect(),
    }
}

/// `<abc@example.com>` → `abc@example.com`.
///
/// Die spitzen Klammern gehören zur Kopfzeilensyntax, nicht zur Kennung. Wer
/// sie stehen lässt, vergleicht irgendwann `<a@b>` mit `a@b` und findet
/// nichts — das ist der Fehler, der Gespräche grundlos aufspaltet.
pub fn saeubere_id(rohdaten: &str) -> String {
    rohdaten
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_owned()
}

/// Trägt der Betreff ein Antwort- oder Weiterleitungspräfix?
pub fn ist_antwort(betreff: &str) -> bool {
    praefix_laenge(betreff.trim_start()) > 0
}

/// Betreff ohne Antwortpräfixe, kleingeschrieben, Leerraum zusammengezogen.
///
/// Mehrfach geschachtelte Präfixe sind der Normalfall, nicht die Ausnahme:
/// `AW: Re: WG: Angebot` entsteht nach drei Stationen in gemischten
/// Postfächern von selbst.
pub fn normalisiere_betreff(betreff: &str) -> String {
    let mut rest = betreff.trim();
    loop {
        let laenge = praefix_laenge(rest);
        if laenge == 0 {
            break;
        }
        rest = rest[laenge..].trim_start();
    }

    rest.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Länge eines führenden Antwortpräfixes in Bytes, oder 0.
///
/// Deutsche Präfixe gehören dazu: In einem deutschen Postfach ist `AW:`
/// häufiger als `Re:`, und `WG:` ist die Weiterleitung. Ein Client, der nur
/// `Re:` kennt, zerreisst hier jeden zweiten Strang.
fn praefix_laenge(rest: &str) -> usize {
    const PRAEFIXE: &[&str] = &[
        "re:", "aw:", "antw:", "antwort:", "fwd:", "fw:", "wg:", "weitergeleitet:", "sv:", "vs:",
    ];

    let klein = rest.to_lowercase();
    for praefix in PRAEFIXE {
        if klein.starts_with(praefix) {
            return praefix.len();
        }
        // `Re[2]:` und `Re(2):` kommen von Outlook und einigen Webmailern.
        if let Some(rumpf) = praefix.strip_suffix(':') {
            for (auf, zu) in [('[', ']'), ('(', ')')] {
                let beginn = format!("{rumpf}{auf}");
                if klein.starts_with(&beginn) {
                    if let Some(ende) = klein[beginn.len()..].find(zu) {
                        let bis = beginn.len() + ende + 1;
                        if klein[bis..].starts_with(':') {
                            return bis + 1;
                        }
                    }
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Gedaechtnis {
        nach_id: HashMap<String, String>,
        nach_betreff: HashMap<String, String>,
    }

    impl BekannteGespraeche for Gedaechtnis {
        fn nach_message_id(&self, message_id: &str) -> Option<String> {
            self.nach_id.get(message_id).cloned()
        }

        fn nach_betreff(&self, betreff: &str) -> Option<String> {
            self.nach_betreff.get(betreff).cloned()
        }
    }

    #[test]
    fn eine_erste_nachricht_eroeffnet_ein_gespraech() {
        let zuordnung = zuordnen(
            Some("<erste@beispiel.de>"),
            None,
            &[],
            "Angebot",
            &Gedaechtnis::default(),
        );
        assert_eq!(zuordnung.schluessel, "erste@beispiel.de");
        assert!(zuordnung.zusammenlegen.is_empty());
    }

    #[test]
    fn eine_antwort_landet_im_gespraech_der_ausgangsmail() {
        let mut gedaechtnis = Gedaechtnis::default();
        gedaechtnis
            .nach_id
            .insert("erste@beispiel.de".into(), "erste@beispiel.de".into());

        let zuordnung = zuordnen(
            Some("<zweite@beispiel.de>"),
            Some("<erste@beispiel.de>"),
            &["<erste@beispiel.de>".into()],
            "Re: Angebot",
            &gedaechtnis,
        );
        assert_eq!(zuordnung.schluessel, "erste@beispiel.de");
    }

    #[test]
    fn spitze_klammern_duerfen_nicht_entscheiden() {
        // Derselbe Fall wie oben, nur schreibt der eine Client die Kennung
        // mit Klammern und der andere ohne. Ohne Säuberung entstehen zwei
        // Gespräche, und die Ursache ist unsichtbar.
        let mut gedaechtnis = Gedaechtnis::default();
        gedaechtnis
            .nach_id
            .insert("erste@beispiel.de".into(), "G1".into());

        let zuordnung = zuordnen(None, Some("erste@beispiel.de"), &[], "Re: X", &gedaechtnis);
        assert_eq!(zuordnung.schluessel, "G1");
    }

    #[test]
    fn zwei_straenge_in_einer_antwort_werden_zusammengelegt() {
        let mut gedaechtnis = Gedaechtnis::default();
        gedaechtnis.nach_id.insert("a@x".into(), "G1".into());
        gedaechtnis.nach_id.insert("b@x".into(), "G2".into());

        let zuordnung = zuordnen(
            Some("<c@x>"),
            Some("<b@x>"),
            &["<a@x>".into(), "<b@x>".into()],
            "Re: beides",
            &gedaechtnis,
        );

        // Welcher der beiden Schlüssel überlebt, entscheidet NICHT das
        // In-Reply-To, sondern eine feste Regel (der kleinere gewinnt). Das
        // ist Absicht: „der Referenzierte gewinnt" liefert bei demselben
        // Zusammenschluss je nach Verarbeitungsreihenfolge einen anderen
        // Schlüssel — und jede gespeicherte Gesprächskennung (eine Auswahl
        // in der Oberfläche, ein Schlummer) zeigte danach ins Leere.
        assert_eq!(zuordnung.schluessel, "G1");
        assert_eq!(zuordnung.zusammenlegen, vec!["G2".to_owned()]);
    }

    #[test]
    fn die_reihenfolge_im_stapel_aendert_nichts() {
        // Genau der Fehler, der die Gesprächsliste aufgespalten hat: Zwei
        // Mails derselben Sekunde, und die Antwort wurde zuerst verarbeitet.
        let erste = Eingang {
            message_id: Some("<erste@x>".into()),
            in_reply_to: None,
            references: Vec::new(),
            betreff: "Angebot".into(),
            gesendet_am: 1_000,
        };
        let antwort = Eingang {
            message_id: Some("<zweite@x>".into()),
            in_reply_to: Some("<erste@x>".into()),
            references: vec!["<erste@x>".into()],
            betreff: "Re: Angebot".into(),
            gesendet_am: 1_000,
        };

        let vorwaerts = gruppieren(
            &[erste.clone(), antwort.clone()],
            &Gedaechtnis::default(),
        );
        let rueckwaerts = gruppieren(&[antwort, erste], &Gedaechtnis::default());

        assert_eq!(vorwaerts.schluessel[0], vorwaerts.schluessel[1], "ein Gespräch");
        assert_eq!(
            rueckwaerts.schluessel[0], rueckwaerts.schluessel[1],
            "auch rückwärts ein Gespräch"
        );
        // Und zwar DASSELBE, nicht nur irgendeines.
        assert_eq!(vorwaerts.schluessel[0], rueckwaerts.schluessel[0]);
        assert_eq!(vorwaerts.schluessel[0], "erste@x", "die Wurzel gibt den Namen");
    }

    #[test]
    fn ein_bestehendes_gespraech_behaelt_seinen_namen() {
        // Eine ältere Nachricht kommt nachträglich dazu (Seite 2 wird
        // nachgeladen). Sie darf das Gespräch nicht umbenennen, auch wenn sie
        // die eigentliche Wurzel ist — sonst wandert die Kennung unter der
        // Oberfläche weg.
        let mut gedaechtnis = Gedaechtnis::default();
        gedaechtnis.nach_id.insert("neu@x".into(), "G9".into());

        let nachtrag = Eingang {
            message_id: Some("<alt@x>".into()),
            in_reply_to: None,
            references: Vec::new(),
            betreff: "Angebot".into(),
            gesendet_am: 10,
        };
        let antwort = Eingang {
            message_id: Some("<noch@x>".into()),
            in_reply_to: Some("<neu@x>".into()),
            references: vec!["<alt@x>".into(), "<neu@x>".into()],
            betreff: "Re: Angebot".into(),
            gesendet_am: 20,
        };

        let gruppierung = gruppieren(&[nachtrag, antwort], &gedaechtnis);
        assert_eq!(gruppierung.schluessel[0], "G9");
        assert_eq!(gruppierung.schluessel[1], "G9");
        assert!(gruppierung.zusammenlegen.is_empty());
    }

    #[test]
    fn ohne_references_hilft_der_betreff_nur_bei_einer_antwort() {
        let mut gedaechtnis = Gedaechtnis::default();
        gedaechtnis
            .nach_betreff
            .insert("angebot".into(), "G7".into());

        // Mit Präfix: Rückfall greift.
        let antwort = zuordnen(Some("<neu@x>"), None, &[], "AW: Angebot", &gedaechtnis);
        assert_eq!(antwort.schluessel, "G7");

        // Ohne Präfix: NICHT greifen. Sonst landet jede neue Mail mit
        // demselben Betreff im selben Gespräch — bei „Rechnung" oder
        // „Anfrage" ein Riesenknäuel.
        let neue = zuordnen(Some("<neu2@x>"), None, &[], "Angebot", &gedaechtnis);
        assert_eq!(neue.schluessel, "neu2@x");
    }

    #[test]
    fn deutsche_und_geschachtelte_praefixe() {
        for (eingabe, erwartet) in [
            ("Re: Angebot", "angebot"),
            ("AW: Angebot", "angebot"),
            ("WG: AW: Re: Angebot", "angebot"),
            ("Re[2]: Angebot", "angebot"),
            ("Re(3): Angebot", "angebot"),
            ("Antwort: Angebot", "angebot"),
            ("Angebot   für   Pflege", "angebot für pflege"),
            ("", ""),
        ] {
            assert_eq!(normalisiere_betreff(eingabe), erwartet, "bei „{eingabe}“");
        }
    }

    #[test]
    fn ein_praefix_im_text_ist_kein_praefix() {
        // „Rechnung" beginnt mit „Re" — wer auf Zeichen statt auf das
        // Doppelpunkt-Muster prüft, schneidet hier zwei Buchstaben ab.
        assert_eq!(normalisiere_betreff("Rechnung 2026-1147"), "rechnung 2026-1147");
        assert!(!ist_antwort("Rechnung 2026-1147"));
    }

    #[test]
    fn ohne_message_id_traegt_der_betreff_den_schluessel() {
        let zuordnung = zuordnen(None, None, &[], "Kontaktformular", &Gedaechtnis::default());
        assert_eq!(zuordnung.schluessel, "betreff:kontaktformular");
    }
}
