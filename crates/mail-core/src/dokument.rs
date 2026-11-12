//! Aus HTML wird ein **Dokument** — und das zeichnet jede Plattform selbst.
//!
//! Der Schnitt ist die eigentliche Entscheidung hinter Aufgabe #9869. Die
//! naheliegenden Wege waren eine eingebettete WebView (ausdrücklich nicht
//! gewollt: Skript-Laufzeit und eigener Netzzugang in der Anzeige fremder
//! Post) oder eine vollständige HTML-Maschine wie Blitz (ein zweites grosses
//! Stück Software, dessen Ausgabe trotzdem nicht in eine native Liste passt).
//!
//! Stattdessen: Der Kern macht aus dem bereinigten HTML eine **flache Folge
//! von Blöcken** mit Textabschnitten. Die trägt alles, was eine Mail
//! braucht — Absätze, Überschriften, Listen, Zitate, Links, Tabellenzeilen —
//! und nichts, was sie nicht braucht. Gezeichnet wird sie mit den
//! Bordmitteln der jeweiligen Plattform: Iced auf dem Desktop, SwiftUI auf
//! iOS, Compose auf Android. Drei Darstellungen, eine Lesart.
//!
//! **Flach und nicht verschachtelt** aus zwei Gründen: Eine Liste zeichnet
//! sich leichter, wenn sie eine Folge von Punkten mit Ebene ist statt eines
//! Baums; und die FFI-Grenze trägt keine rekursiven Aufzählungen ohne
//! Umschweife.

use std::rc::Rc;

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// Ein Stück Text mit seiner Auszeichnung.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Abschnitt {
    pub text: String,
    pub fett: bool,
    pub kursiv: bool,
    /// Zieladresse, falls der Abschnitt ein Verweis ist.
    pub verweis: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Block {
    Absatz {
        abschnitte: Vec<Abschnitt>,
        /// Wie tief zitiert. Mail-Verläufe stapeln das gern vierfach.
        zitatebene: u32,
    },
    Ueberschrift {
        stufe: u32,
        abschnitte: Vec<Abschnitt>,
    },
    Listenpunkt {
        abschnitte: Vec<Abschnitt>,
        ebene: u32,
        /// Bei nummerierten Listen die Nummer, sonst leer.
        nummer: Option<u32>,
    },
    Trennlinie,
    Bild {
        /// `cid:…` für Inline-Bilder, sonst die ursprüngliche Adresse.
        quelle: String,
        alt: String,
    },
    Tabellenzeile {
        zellen: Vec<String>,
        kopfzeile: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Dokument {
    pub bloecke: Vec<Block>,
}

impl Dokument {
    /// Der reine Text des Dokuments — für Vorschauzeilen und die Suche.
    pub fn als_text(&self) -> String {
        let mut zeilen = Vec::new();
        for block in &self.bloecke {
            match block {
                Block::Absatz { abschnitte, .. } | Block::Ueberschrift { abschnitte, .. } => {
                    zeilen.push(zusammenziehen(abschnitte));
                }
                Block::Listenpunkt { abschnitte, .. } => {
                    zeilen.push(format!("• {}", zusammenziehen(abschnitte)));
                }
                Block::Tabellenzeile { zellen, .. } => zeilen.push(zellen.join(" | ")),
                Block::Bild { alt, .. } if !alt.is_empty() => zeilen.push(format!("[{alt}]")),
                Block::Bild { .. } | Block::Trennlinie => {}
            }
        }
        zeilen.retain(|zeile| !zeile.trim().is_empty());
        zeilen.join("\n")
    }
}

fn zusammenziehen(abschnitte: &[Abschnitt]) -> String {
    abschnitte
        .iter()
        .map(|abschnitt| abschnitt.text.as_str())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Wandelt (bereits bereinigtes) HTML in ein Dokument.
pub fn aus_html(html: &str) -> Dokument {
    let dom = html5ever::parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .unwrap_or_else(|_| {
            html5ever::parse_document(RcDom::default(), Default::default())
                .from_utf8()
                .read_from(&mut "".as_bytes())
                .expect("leeres Dokument")
        });

    let mut bauer = Bauer::default();
    bauer.knoten(&dom.document, Stil::default());
    bauer.absatz_abschliessen();
    Dokument { bloecke: bauer.bloecke }
}

#[derive(Debug, Clone, Default)]
struct Stil {
    fett: bool,
    kursiv: bool,
    verweis: Option<String>,
    listenebene: u32,
    /// Nummer des nächsten Punkts, falls in einer nummerierten Liste.
    nummer: Option<u32>,
    /// In `<pre>` bleiben Leerzeichen und Umbrüche stehen.
    vorformatiert: bool,
}

#[derive(Default)]
struct Bauer {
    bloecke: Vec<Block>,
    offen: Vec<Abschnitt>,
    zitatebene: u32,
}

impl Bauer {
    fn text_anfuegen(&mut self, text: &str, stil: &Stil) {
        if stil.vorformatiert {
            self.stueck_anfuegen(text.to_owned(), stil);
            return;
        }

        // Leerraum zusammenziehen wie HTML es tut — aber führenden und
        // folgenden getrennt behandeln. Wer beides mit `trim` wegwirft, klebt
        // `Das ist` und `wichtig` zusammen, sobald zwischen ihnen eine
        // Auszeichnung steht. Das sieht man erst im fertigen Text, und dann
        // sucht man es im Zeichner statt hier.
        let fuehrend = text.starts_with(char::is_whitespace);
        let folgend = text.ends_with(char::is_whitespace);
        let kern = text.split_whitespace().collect::<Vec<_>>().join(" ");

        if kern.is_empty() {
            // Reiner Leerraum zwischen zwei Marken: genau ein Leerzeichen,
            // und nur wenn davor schon etwas steht.
            if (fuehrend || folgend) && self.endet_mit_zeichen() {
                self.stueck_anfuegen(" ".to_owned(), stil);
            }
            return;
        }

        let mut zusammengesetzt = String::with_capacity(kern.len() + 2);
        if fuehrend && self.endet_mit_zeichen() {
            zusammengesetzt.push(' ');
        }
        zusammengesetzt.push_str(&kern);
        if folgend {
            zusammengesetzt.push(' ');
        }

        self.stueck_anfuegen(zusammengesetzt, stil);
    }

    /// Steht am Ende des bisher Gesammelten ein Zeichen, das kein Leerraum
    /// ist? Nur dann ist ein trennendes Leerzeichen sinnvoll.
    fn endet_mit_zeichen(&self) -> bool {
        self.offen
            .last()
            .and_then(|abschnitt| abschnitt.text.chars().last())
            .is_some_and(|zeichen| !zeichen.is_whitespace())
    }

    fn stueck_anfuegen(&mut self, text: String, stil: &Stil) {
        if text.is_empty() {
            return;
        }

        // Gleich ausgezeichnete Nachbarn zusammenfassen: Sonst entsteht aus
        // `<b>Hallo</b><b> Welt</b>` zweimal dasselbe, und die Darstellung
        // bekommt doppelt so viele Stücke wie nötig.
        if let Some(letzter) = self.offen.last_mut() {
            if letzter.fett == stil.fett
                && letzter.kursiv == stil.kursiv
                && letzter.verweis == stil.verweis
            {
                letzter.text.push_str(&text);
                return;
            }
        }

        self.offen.push(Abschnitt {
            text,
            fett: stil.fett,
            kursiv: stil.kursiv,
            verweis: stil.verweis.clone(),
        });
    }

    fn absatz_abschliessen(&mut self) {
        if self.offen.iter().all(|abschnitt| abschnitt.text.trim().is_empty()) {
            self.offen.clear();
            return;
        }
        let abschnitte = std::mem::take(&mut self.offen);
        let zitatebene = self.zitatebene;
        self.bloecke.push(Block::Absatz { abschnitte, zitatebene });
    }

    fn block_abschliessen(&mut self, block: Block) {
        self.absatz_abschliessen();
        self.bloecke.push(block);
    }

    fn knoten(&mut self, knoten: &Handle, stil: Stil) {
        match &knoten.data {
            NodeData::Text { contents } => {
                let text = contents.borrow().to_string();
                self.text_anfuegen(&text, &stil);
            }
            NodeData::Element { name, attrs, .. } => {
                let marke = name.local.to_ascii_lowercase().to_string();
                let attribut = |gesucht: &str| -> Option<String> {
                    attrs
                        .borrow()
                        .iter()
                        .find(|attribut| &*attribut.name.local == gesucht)
                        .map(|attribut| attribut.value.to_string())
                };

                match marke.as_str() {
                    // Was im Kopf steht, gehört nicht in den Text. `<style>`
                    // und `<title>` sind Textknoten — ohne diese Zeile stünde
                    // das Stylesheet mitten in der Mail.
                    "head" | "style" | "title" | "script" => return,

                    "br" => {
                        self.absatz_abschliessen();
                        return;
                    }
                    "hr" => {
                        self.block_abschliessen(Block::Trennlinie);
                        return;
                    }
                    "img" => {
                        let quelle = attribut("src").unwrap_or_default();
                        let alt = attribut("alt").unwrap_or_default();
                        if !quelle.is_empty() || !alt.is_empty() {
                            self.block_abschliessen(Block::Bild { quelle, alt });
                        }
                        return;
                    }

                    "p" | "div" => self.absatz_abschliessen(),

                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        self.absatz_abschliessen();
                        let stufe = marke[1..].parse::<u32>().unwrap_or(1);
                        for kind in knoten.children.borrow().iter() {
                            self.knoten(kind, stil.clone());
                        }
                        let abschnitte = std::mem::take(&mut self.offen);
                        if !abschnitte.is_empty() {
                            self.bloecke.push(Block::Ueberschrift { stufe, abschnitte });
                        }
                        return;
                    }

                    "blockquote" => {
                        self.absatz_abschliessen();
                        self.zitatebene += 1;
                        for kind in knoten.children.borrow().iter() {
                            self.knoten(kind, stil.clone());
                        }
                        self.absatz_abschliessen();
                        self.zitatebene = self.zitatebene.saturating_sub(1);
                        return;
                    }

                    "ul" | "ol" => {
                        self.absatz_abschliessen();
                        let mut innen = stil.clone();
                        innen.listenebene += 1;
                        innen.nummer = (marke == "ol").then_some(1);
                        for kind in knoten.children.borrow().iter() {
                            self.knoten(kind, innen.clone());
                            if let Some(nummer) = innen.nummer.as_mut() {
                                if ist_marke(kind, "li") {
                                    *nummer += 1;
                                }
                            }
                        }
                        return;
                    }

                    "li" => {
                        self.absatz_abschliessen();
                        for kind in knoten.children.borrow().iter() {
                            self.knoten(kind, stil.clone());
                        }
                        let abschnitte = std::mem::take(&mut self.offen);
                        if !abschnitte.is_empty() {
                            self.bloecke.push(Block::Listenpunkt {
                                abschnitte,
                                ebene: stil.listenebene.max(1),
                                nummer: stil.nummer,
                            });
                        }
                        return;
                    }

                    "tr" => {
                        self.absatz_abschliessen();
                        let mut zellen = Vec::new();
                        let mut kopfzeile = false;
                        for kind in knoten.children.borrow().iter() {
                            if ist_marke(kind, "td") || ist_marke(kind, "th") {
                                if ist_marke(kind, "th") {
                                    kopfzeile = true;
                                }
                                let mut zellenbauer = Bauer::default();
                                zellenbauer.knoten(kind, stil.clone());
                                zellenbauer.absatz_abschliessen();
                                zellen.push(Dokument { bloecke: zellenbauer.bloecke }.als_text());
                            }
                        }
                        if zellen.iter().any(|zelle| !zelle.trim().is_empty()) {
                            self.bloecke.push(Block::Tabellenzeile { zellen, kopfzeile });
                        }
                        return;
                    }

                    _ => {}
                }

                let mut innen = stil.clone();
                match marke.as_str() {
                    "b" | "strong" => innen.fett = true,
                    "i" | "em" => innen.kursiv = true,
                    "pre" | "code" => innen.vorformatiert = true,
                    "a" => innen.verweis = attribut("href"),
                    _ => {}
                }

                for kind in knoten.children.borrow().iter() {
                    self.knoten(kind, innen.clone());
                }

                if matches!(marke.as_str(), "p" | "div") {
                    self.absatz_abschliessen();
                }
            }
            NodeData::Document => {
                for kind in knoten.children.borrow().iter() {
                    self.knoten(kind, stil.clone());
                }
            }
            _ => {
                for kind in knoten.children.borrow().iter() {
                    self.knoten(kind, stil.clone());
                }
            }
        }
    }
}

fn ist_marke(knoten: &Rc<markup5ever_rcdom::Node>, gesucht: &str) -> bool {
    matches!(&knoten.data, NodeData::Element { name, .. }
        if name.local.to_ascii_lowercase().as_ref() as &str == gesucht)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_von(block: &Block) -> String {
        match block {
            Block::Absatz { abschnitte, .. }
            | Block::Ueberschrift { abschnitte, .. }
            | Block::Listenpunkt { abschnitte, .. } => zusammenziehen(abschnitte),
            Block::Tabellenzeile { zellen, .. } => zellen.join(" | "),
            Block::Bild { alt, .. } => alt.clone(),
            Block::Trennlinie => "—".to_owned(),
        }
    }

    #[test]
    fn absaetze_bleiben_absaetze() {
        let dokument = aus_html("<p>Guten Tag,</p><p>anbei das Angebot.</p>");
        assert_eq!(dokument.bloecke.len(), 2);
        assert_eq!(text_von(&dokument.bloecke[0]), "Guten Tag,");
        assert_eq!(text_von(&dokument.bloecke[1]), "anbei das Angebot.");
    }

    #[test]
    fn fett_und_kursiv_kommen_als_auszeichnung_an() {
        let dokument = aus_html("<p>Das ist <b>wichtig</b> und <i>schräg</i>.</p>");
        let Block::Absatz { abschnitte, .. } = &dokument.bloecke[0] else {
            panic!("kein Absatz");
        };

        let fett = abschnitte.iter().find(|a| a.fett).expect("nichts fett");
        assert_eq!(fett.text, "wichtig");
        let kursiv = abschnitte.iter().find(|a| a.kursiv).expect("nichts kursiv");
        assert_eq!(kursiv.text, "schräg");
        // Und der Text bleibt vollständig.
        assert_eq!(zusammenziehen(abschnitte), "Das ist wichtig und schräg.");
    }

    #[test]
    fn ein_verweis_traegt_sein_ziel_mit() {
        let dokument = aus_html(r#"<p>Siehe <a href="https://beispiel.de/angebot">Angebot</a>.</p>"#);
        let Block::Absatz { abschnitte, .. } = &dokument.bloecke[0] else {
            panic!("kein Absatz");
        };
        let verweis = abschnitte
            .iter()
            .find(|a| a.verweis.is_some())
            .expect("kein Verweis");
        assert_eq!(verweis.text, "Angebot");
        assert_eq!(verweis.verweis.as_deref(), Some("https://beispiel.de/angebot"));
    }

    #[test]
    fn ueberschriften_behalten_ihre_stufe() {
        let dokument = aus_html("<h1>Gross</h1><h3>Kleiner</h3>");
        assert!(matches!(dokument.bloecke[0], Block::Ueberschrift { stufe: 1, .. }));
        assert!(matches!(dokument.bloecke[1], Block::Ueberschrift { stufe: 3, .. }));
    }

    #[test]
    fn listen_werden_zu_punkten_mit_ebene_und_nummer() {
        let dokument = aus_html("<ul><li>eins</li><li>zwei</li></ul>");
        let punkte: Vec<&Block> = dokument
            .bloecke
            .iter()
            .filter(|b| matches!(b, Block::Listenpunkt { .. }))
            .collect();
        assert_eq!(punkte.len(), 2);
        assert_eq!(text_von(punkte[0]), "eins");

        let nummeriert = aus_html("<ol><li>erstens</li><li>zweitens</li></ol>");
        let nummern: Vec<Option<u32>> = nummeriert
            .bloecke
            .iter()
            .filter_map(|b| match b {
                Block::Listenpunkt { nummer, .. } => Some(*nummer),
                _ => None,
            })
            .collect();
        assert_eq!(nummern, vec![Some(1), Some(2)]);
    }

    #[test]
    fn ein_zitat_traegt_seine_ebene() {
        let dokument = aus_html("<p>Ich schrieb:</p><blockquote><p>Hallo</p></blockquote>");
        let ebenen: Vec<u32> = dokument
            .bloecke
            .iter()
            .filter_map(|b| match b {
                Block::Absatz { zitatebene, .. } => Some(*zitatebene),
                _ => None,
            })
            .collect();
        assert_eq!(ebenen, vec![0, 1], "Zitatebene nicht erkannt");
    }

    #[test]
    fn tabellenzeilen_bleiben_zeilen() {
        let dokument = aus_html(
            "<table><tr><th>Posten</th><th>Preis</th></tr><tr><td>Pflege</td><td>120 €</td></tr></table>",
        );
        let zeilen: Vec<&Block> = dokument
            .bloecke
            .iter()
            .filter(|b| matches!(b, Block::Tabellenzeile { .. }))
            .collect();
        assert_eq!(zeilen.len(), 2);
        assert!(matches!(zeilen[0], Block::Tabellenzeile { kopfzeile: true, .. }));
        assert_eq!(text_von(zeilen[1]), "Pflege | 120 €");
    }

    #[test]
    fn das_stylesheet_landet_nicht_im_text() {
        // Der Fehler, den fast jede selbstgebaute HTML-zu-Text-Umwandlung
        // macht: `<style>` ist ein Textknoten, und sein Inhalt steht dann
        // mitten in der Mail.
        let dokument = aus_html(
            "<html><head><style>.a{color:red}</style><title>Egal</title></head>\
             <body><p>Guten Tag</p></body></html>",
        );
        let text = dokument.als_text();
        assert!(!text.contains("color"), "Stylesheet im Text: {text:?}");
        assert!(!text.contains("Egal"), "Titel im Text: {text:?}");
        assert_eq!(text, "Guten Tag");
    }

    #[test]
    fn kaputtes_html_bricht_nicht() {
        // Mail-HTML ist regelmässig kaputt. Nichts davon darf zu einer
        // Panik führen — ein Absturz beim Öffnen einer Mail ist das
        // Schlimmste, was ein Mailprogramm tun kann.
        for kaputt in [
            "<p>offen",
            "<b><i>verschachtelt falsch</b></i>",
            "<table><tr><td>ohne Ende",
            "<<<>>>",
            "",
            "<div><div><div><p>tief</p>",
        ] {
            let dokument = aus_html(kaputt);
            let _ = dokument.als_text();
        }
    }

    #[test]
    fn leerraum_wird_zusammengezogen_aber_nicht_verschluckt() {
        let dokument = aus_html("<p>Guten   Tag,\n   wie geht es?</p>");
        assert_eq!(text_von(&dokument.bloecke[0]), "Guten Tag, wie geht es?");

        // Zwischen zwei Auszeichnungen darf das Leerzeichen nicht wegfallen.
        let mit_auszeichnung = aus_html("<p><b>Angebot</b> <i>1147</i></p>");
        assert_eq!(text_von(&mit_auszeichnung.bloecke[0]), "Angebot 1147");
    }

    #[test]
    fn ein_bild_wird_zum_eigenen_block() {
        let dokument = aus_html(r#"<p>Logo:</p><img src="cid:logo@firma" alt="Firmenlogo">"#);
        let bild = dokument
            .bloecke
            .iter()
            .find_map(|b| match b {
                Block::Bild { quelle, alt } => Some((quelle.clone(), alt.clone())),
                _ => None,
            })
            .expect("kein Bild");
        assert_eq!(bild.0, "cid:logo@firma");
        assert_eq!(bild.1, "Firmenlogo");
    }

    #[test]
    fn br_trennt_zeilen() {
        let dokument = aus_html("<p>Erste Zeile<br>Zweite Zeile</p>");
        assert_eq!(dokument.als_text(), "Erste Zeile\nZweite Zeile");
    }
}
