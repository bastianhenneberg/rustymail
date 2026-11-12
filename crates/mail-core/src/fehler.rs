//! Die Fehler, die über die FFI-Grenze gehen.
//!
//! Sie sind absichtlich grobkörnig: Eine Oberfläche muss unterscheiden
//! können, ob sie einen Wiederholen-Knopf, eine Anmeldemaske oder eine
//! Entschuldigung zeigt. Mehr Fälle helfen ihr nicht, kosten aber in jeder
//! der drei Oberflächen eine Verzweigung.

use thiserror::Error;

#[derive(Debug, Error)]
#[cfg_attr(feature = "ffi", derive(uniffi::Error))]
pub enum Fehler {
    /// Server nicht erreichbar, Zeitüberschreitung, Netz weg. Wiederholbar.
    #[error("Postfach nicht erreichbar: {grund}")]
    NichtErreichbar { grund: String },

    /// Zugangsdaten abgelehnt oder Token abgelaufen. Braucht einen Menschen.
    #[error("Anmeldung abgelehnt: {grund}")]
    AnmeldungAbgelehnt { grund: String },

    /// Die Mitte (AI Brain) antwortet nicht. Der letzte gute Stand gilt weiter,
    /// aber Schreibvorgänge dorthin müssen warten.
    #[error("Die Mitte antwortet nicht: {grund}")]
    MitteStumm { grund: String },

    /// Lokaler Speicher kaputt oder nicht beschreibbar.
    #[error("Lokaler Speicher: {grund}")]
    Speicher { grund: String },

    /// Angefragt wurde etwas, das es nicht (mehr) gibt — gelöschte Nachricht,
    /// umbenannter Ordner, abgelaufene Kennung.
    #[error("Nicht gefunden: {was}")]
    NichtGefunden { was: String },

    /// Noch nicht gebaut. Steht hier, damit das Gerüst ehrlich ist, statt mit
    /// `todo!()` zu panikken — eine Panik über die FFI-Grenze ist
    /// undefiniertes Verhalten und auf dem Gerät ein Absturz ohne Ursache.
    #[error("Noch nicht gebaut: {was}")]
    NochNichtGebaut { was: String },
}

pub type Ergebnis<T> = Result<T, Fehler>;

impl From<mail_proto::Protokollfehler> for Fehler {
    fn from(fehler: mail_proto::Protokollfehler) -> Self {
        use mail_proto::Protokollfehler as P;
        match fehler {
            P::NichtErreichbar(grund) => Self::NichtErreichbar { grund },
            P::AnmeldungAbgelehnt(grund) => Self::AnmeldungAbgelehnt { grund },
            P::Unerwartet(grund) => Self::NichtErreichbar {
                grund: format!("unerwartete Antwort: {grund}"),
            },
            P::NochNichtGebaut(was) => Self::NochNichtGebaut { was: was.to_owned() },
        }
    }
}

impl Fehler {
    pub(crate) fn offen(was: &str) -> Self {
        Self::NochNichtGebaut { was: was.to_owned() }
    }
}
