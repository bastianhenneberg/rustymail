//! Die ViewModels — das, was über die Grenze nach draußen geht.
//!
//! Grobkörnig mit Absicht. Ein Kern, der `betreff(id)` und `absender(id)`
//! einzeln anbietet, wird von der Oberfläche pro Zeile dreimal gerufen; bei
//! 200 Zeilen sind das 600 Grenzübertritte für einen Bildschirm. Hier geht
//! stattdessen die fertige Zeile raus.
//!
//! Zeitpunkte sind Unix-Millisekunden (`i64`), keine Datumstypen: Swift und
//! Kotlin haben eigene, und eine Zeitzone, die über die Grenze wandert, ist
//! eine Fehlerquelle ohne Gegenwert.

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct KontoAnsicht {
    pub id: String,
    pub beschriftung: String,
    pub adresse: String,
    pub gesundheit: Gesundheit,
    pub ungelesen: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Gesundheit {
    Gut,
    /// Erreichbar, aber mit Vorbehalt — etwa ein Token, das bald abläuft.
    Wacklig { grund: String },
    Kaputt { grund: String },
    /// Noch nie verbunden.
    Unbekannt,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct OrdnerAnsicht {
    pub id: String,
    pub pfad: String,
    pub beschriftung: String,
    /// Posteingang, Gesendet, Papierkorb … — die Oberfläche soll Systemordner
    /// nicht an deutschen Namen erraten müssen.
    pub rolle: Ordnerrolle,
    pub gesamt: u32,
    pub ungelesen: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Ordnerrolle {
    Eingang,
    Gesendet,
    Entwuerfe,
    Papierkorb,
    Spam,
    Archiv,
    Sonstiger,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ThreadZeile {
    pub thread_id: String,
    pub betreff: String,
    /// Wer im Gespräch vorkommt, in der Reihenfolge des Auftretens.
    pub beteiligte: Vec<String>,
    pub vorschau: String,
    pub letzte_nachricht_am: i64,
    pub anzahl: u32,
    pub ungelesen: bool,
    pub markiert: bool,
    pub hat_anhang: bool,
    /// Bis wann geschlummert, falls geschlummert.
    pub schlummert_bis: Option<i64>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct NachrichtAnsicht {
    pub id: String,
    pub thread_id: String,
    pub betreff: String,
    pub von: Adresse,
    pub an: Vec<Adresse>,
    pub kopie: Vec<Adresse>,
    pub blindkopie: Vec<Adresse>,
    pub gesendet_am: i64,
    /// Der Textkörper, bereits aufbereitet: Inline-Bilder sind durch
    /// `cid:`-Ersetzung auf lokale Kennungen umgeschrieben, nicht auf
    /// Fremdadressen. Was nach aussen zeigt, zeigt nach aussen erst, wenn
    /// die Person es erlaubt.
    pub koerper: Koerper,
    pub anhaenge: Vec<AnhangAnsicht>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Adresse {
    pub name: Option<String>,
    pub adresse: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Koerper {
    /// Reiner Text. Immer vorhanden, notfalls aus dem HTML gewonnen.
    Text { inhalt: String },
    /// Ein zerlegtes HTML-Dokument: eine flache Folge von Blöcken, die
    /// jede Plattform mit ihren eigenen Mitteln zeichnet. Kein HTML mehr,
    /// keine WebView — siehe `dokument.rs`.
    Html {
        dokument: crate::dokument::Dokument,
        /// Reiner Text derselben Nachricht, für Vorschau und Suche.
        text_ersatz: String,
    },
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AnhangAnsicht {
    pub id: String,
    pub dateiname: String,
    pub medientyp: String,
    pub groesse: u64,
    /// Inline-Bilder stehen im Textkörper und gehören nicht in die
    /// Anhangsliste darunter.
    pub inline: bool,
    /// Die Content-Id, auf die der Textkörper per `cid:` verweist.
    pub content_id: Option<String>,
}

impl NachrichtAnsicht {
    /// Der darstellbare Text, egal welcher Teil vorliegt.
    ///
    /// Steht hier und nicht in der Oberfläche: Sonst baut jede der drei
    /// Plattformen die Fallunterscheidung nach, und eine davon vergisst den
    /// HTML-Fall — bei Newslettern bliebe die Anzeige dort leer.
    pub fn koerper_text(&self) -> String {
        match &self.koerper {
            Koerper::Text { inhalt } => inhalt.clone(),
            Koerper::Html { text_ersatz, .. } => text_ersatz.clone(),
        }
    }
}

/// Eine Seite Gesprächszeilen samt Marke für die nächste. Keine Seitenzahlen:
/// IMAP-Ordner verändern sich unter der Hand, eine Marke überlebt das.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ThreadSeite {
    pub zeilen: Vec<ThreadZeile>,
    pub weitere_marke: Option<String>,
}
