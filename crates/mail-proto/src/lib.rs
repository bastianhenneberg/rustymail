//! Die Protokollschicht: ein Vertrag, zwei Erfüllungen.
//!
//! IMAP und JMAP liefern dieselben Dinge auf unvereinbaren Wegen. IMAP ist
//! ordnerbasiert, zählt mit UIDs und kennt keinen Begriff von „Gespräch";
//! JMAP ist mengenbasiert, hat Thread-Kennungen und einen Zustandsstempel für
//! alles. Ein Kern, der beides direkt anspricht, bekommt an jeder Stelle eine
//! Verzweigung. Deshalb steht hier `Kontobackend` davor.
//!
//! Der Vertrag ist nach IMAP geschnitten, weil IMAP das schwächere Modell ist:
//! Was IMAP kann, kann JMAP nachbilden — umgekehrt nicht.

use async_trait::async_trait;

pub mod imap;
pub mod jmap;
pub mod sasl;
pub mod tls;
pub mod zeitpunkt;

#[derive(Debug, Clone)]
pub struct Zugang {
    pub host: String,
    pub port: u16,
    pub benutzer: String,
    /// Passwort oder OAuth2-Zugriffstoken. Kommt vom `Tresor` der Plattform
    /// und wird hier nur durchgereicht, nie abgelegt.
    pub geheimnis: String,
    pub verschluesselung: Verschluesselung,
    /// Wie angemeldet wird. Entscheidet, wie `geheimnis` zu lesen ist.
    pub anmeldeart: Anmeldeart,
    /// Wohin gesendet wird. Eigene Angabe, weil Postausgang und Posteingang
    /// bei fast jedem Anbieter auf verschiedenen Rechnern liegen
    /// (`imap.example.de` gegen `smtp.example.de`) — und bei manchen auf
    /// verschiedenen Ports desselben.
    pub senden: Option<Sendeziel>,
}

#[derive(Debug, Clone)]
pub struct Sendeziel {
    pub host: String,
    pub port: u16,
    pub verschluesselung: Verschluesselung,
}

/// Wie sich der Client ausweist.
///
/// Der Unterschied ist nicht bloss ein anderer Befehl: Bei `Passwort` steht
/// im Tresor ein Passwort, das jahrelang gilt. Bei `XOauth2` steht dort ein
/// **Zugriffstoken**, das nach einer Stunde abläuft und erneuert werden muss
/// — wer das übersieht, baut einen Client, der nach einer Stunde stumm
/// aufhört zu arbeiten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anmeldeart {
    #[default]
    Passwort,
    /// SASL XOAUTH2. Das, was Gmail und Outlook verlangen — beide nehmen
    /// seit Jahren kein gewöhnliches Passwort mehr an.
    XOauth2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verschluesselung {
    /// Implizites TLS, Regelfall (993 / 465).
    Tls,
    /// Klartext mit anschließendem STARTTLS (143 / 587).
    Starttls,
}

#[derive(Debug, Clone)]
pub struct Ordner {
    /// Die Kennung, unter der der Server den Ordner anspricht. Bei IMAP der
    /// Pfad, bei JMAP die Mailbox-Id — deshalb eine Zeichenkette und keine
    /// Struktur.
    pub pfad: String,
    /// Der anzuzeigende Name. Bei IMAP der letzte Pfadteil, bei JMAP der
    /// Name der Mailbox.
    pub name: String,
    pub trenner: char,
    pub gesamt: u32,
    pub ungelesen: u32,
    /// Was der Server selbst über die Rolle sagt (SPECIAL-USE bei IMAP,
    /// `role` bei JMAP). Nicht aus dem Namen erraten: ein Ordner „Archiv"
    /// kann ein gewöhnlicher Ordner sein, und „[Gmail]/All Mail" ist keiner.
    pub rolle: Option<String>,
    /// IMAP-UIDVALIDITY. Ändert sie sich, ist jede gespeicherte UID dieses
    /// Ordners wertlos und der Ordner muss neu gelesen werden.
    pub uidvalidity: Option<u32>,
    pub highestmodseq: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Kopfzeilen {
    /// Die Kennung der Nachricht beim Server.
    ///
    /// Zeichenkette und nicht Zahl: IMAP hat UIDs, JMAP hat `Email/id` —
    /// und eine erfundene Zahl fuer JMAP waere eine Luege, die erst beim
    /// zweiten Abgleich auffliegt.
    pub kennung: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub betreff: String,
    pub von: String,
    pub gesendet_am: i64,
    pub groesse: u32,
    pub flaggen: Vec<Flagge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flagge {
    Gelesen,
    Markiert,
    Beantwortet,
    Weitergeleitet,
    Entwurf,
    Geloescht,
    /// Alles, was der Server als eigenes Schlüsselwort führt.
    Eigen(String),
}

#[derive(Debug, Clone)]
pub struct Seite {
    pub kopfzeilen: Vec<Kopfzeilen>,
    /// Marke für die nächste Seite — oder `None`, wenn der Ordner durch ist.
    pub weiter_ab: Option<String>,
    /// Die UIDVALIDITY, die beim Lesen galt.
    ///
    /// Sie gehört an die Seite und nicht bloss in die Ordnerliste: Zwischen
    /// „Ordner auflisten" und „Kopfzeilen holen" kann sie sich ändern, und
    /// dann beschreiben die gelieferten UIDs einen anderen Bestand als den,
    /// den der Aufrufer gespeichert hat.
    pub uidvalidity: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum Protokollfehler {
    #[error("nicht erreichbar: {0}")]
    NichtErreichbar(String),
    #[error("Anmeldung abgelehnt: {0}")]
    AnmeldungAbgelehnt(String),
    #[error("Server antwortet unerwartet: {0}")]
    Unerwartet(String),
    #[error("noch nicht gebaut: {0}")]
    NochNichtGebaut(&'static str),
}

pub type Ergebnis<T> = Result<T, Protokollfehler>;

/// Was der Kern von einem Postfach braucht — und nicht mehr.
#[async_trait]
pub trait Kontobackend: Send + Sync {
    async fn ordner(&self) -> Ergebnis<Vec<Ordner>>;

    /// Kopfzeilen ab `ab_uid` absteigend, höchstens `anzahl`. Absteigend,
    /// weil jede Oberfläche mit der neuesten Nachricht anfängt.
    async fn kopfzeilen(&self, pfad: &str, marke: Option<&str>, anzahl: u32) -> Ergebnis<Seite>;

    /// Die vollständige Nachricht als RFC-5322-Rohfassung. Geparst wird
    /// einmal, im Kern, mit `mail-parser` — nicht zweimal verschieden.
    async fn rohfassung(&self, pfad: &str, kennung: &str) -> Ergebnis<Vec<u8>>;

    async fn flaggen_setzen(&self, pfad: &str, kennung: &str, setzen: &[Flagge], loeschen: &[Flagge]) -> Ergebnis<()>;

    async fn verschieben(&self, von: &str, kennung: &str, nach: &str) -> Ergebnis<()>;

    /// Eine Nachricht in einen Ordner legen (IMAP APPEND).
    ///
    /// `empfangen_am` MUSS mitgegeben werden: Ohne internaldate setzt der
    /// Server den Zeitpunkt des Einlegens, und jede kopierte Nachricht trägt
    /// dann das Datum des Umzugstags statt ihres eigenen.
    ///
    /// Gibt KEINE Kennung zurück: Ohne die Erweiterung UIDPLUS verrät der
    /// Server die UID der eingelegten Nachricht nicht, und eine erfundene
    /// wäre schlimmer als keine.
    async fn anhaengen(&self, pfad: &str, roh: &[u8], empfangen_am: i64, flaggen: &[Flagge]) -> Ergebnis<()>;

    /// Verschickt eine fertige Nachricht.
    ///
    /// `absender` ist der Umschlag-Absender (SMTP `MAIL FROM`), nicht die
    /// `From:`-Kopfzeile. Die beiden dürfen auseinandergehen — bei
    /// Verteilern und Weiterleitungen tun sie das regelmässig — und ein
    /// Server, der prüft, prüft den Umschlag.
    async fn senden(&self, roh: &[u8], absender: &str, empfaenger: &[String]) -> Ergebnis<()>;
}
