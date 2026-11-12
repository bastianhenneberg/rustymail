//! Konten anlegen, auflisten, entfernen.
//!
//! Die Zugangsdaten sind hier in zwei Teile geschnitten, und das ist der
//! wichtigste Satz dieser Datei: **`Verbindungsdaten` wandern in die
//! Datenbank, das Geheimnis wandert in den Tresor.** Beides zusammen gibt es
//! nur für die Dauer einer Verbindung, im Speicher, nie auf der Platte.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Protokoll {
    Imap,
    Jmap,
}

impl Protokoll {
    pub(crate) fn als_text(self) -> &'static str {
        match self {
            Self::Imap => "imap",
            Self::Jmap => "jmap",
        }
    }

    pub(crate) fn aus_text(text: &str) -> Self {
        match text {
            "jmap" => Self::Jmap,
            // Unbekanntes als IMAP zu lesen ist die harmlosere Richtung: Ein
            // IMAP-Versuch gegen einen JMAP-Server scheitert sichtbar, und
            // niemand verliert dadurch Daten.
            _ => Self::Imap,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Verschluesselung {
    /// Implizites TLS, der Regelfall (993 / 465).
    Tls,
    /// Klartext mit anschließendem STARTTLS (143 / 587).
    Starttls,
}

/// Alles, was zum Verbinden nötig ist — **außer** dem Geheimnis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Verbindungsdaten {
    pub host: String,
    pub port: u16,
    pub benutzer: String,
    pub verschluesselung: Verschluesselung,
    /// Nur bei JMAP: die Adresse der Sitzungsressource.
    pub jmap_sitzung: Option<String>,
    /// Postausgang. Leer heisst: derselbe Rechner wie der Posteingang — das
    /// stimmt bei manchen Anbietern und bei vielen nicht.
    ///
    /// `serde(default)` ist Pflicht und kein Beiwerk: Die Verbindungsdaten
    /// liegen als JSON in der Datenbank, und ein Konto, das vor dieser
    /// Erweiterung angelegt wurde, hat das Feld nicht. Ohne Vorgabewert
    /// liesse sich so ein Konto nicht mehr lesen — und zwar genau einmal,
    /// beim ersten Start nach dem Update.
    #[serde(default)]
    pub smtp_host: Option<String>,
    #[serde(default)]
    pub smtp_port: Option<u16>,
}

/// Was die Oberfläche beim Anlegen übergibt. Das Geheimnis steht hier ein
/// einziges Mal — danach existiert es nur noch im Tresor.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Kontoentwurf {
    pub beschriftung: String,
    pub adresse: String,
    pub protokoll: Protokoll,
    pub verbindung: Verbindungsdaten,
    pub geheimnis: String,
}

/// Der Name, unter dem das Geheimnis eines Kontos im Tresor liegt.
///
/// Als Funktion und nicht als Zeichenkette an drei Stellen: Beim Entfernen
/// muss derselbe Name entstehen wie beim Anlegen, sonst bleibt ein Eintrag im
/// Schlüsselbund des Benutzers zurück, den niemand mehr zuordnen kann.
pub(crate) fn tresor_kennung(konto_id: &str) -> String {
    format!("konto:{konto_id}:zugang")
}
