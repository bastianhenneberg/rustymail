//! Zugangsdaten gehören nicht in den Kern.
//!
//! Der Kern braucht ein Passwort oder Token für die Dauer einer Verbindung
//! und danach nicht mehr. Wo es zwischen zwei Starts liegt, entscheidet die
//! Plattform: iOS Keychain, Android Keystore, auf dem Linux-Desktop der
//! Secret Service. Hielte der Kern es selbst, läge es in einer Datei, die
//! jede andere App auf dem Rechner lesen kann — und ein Speicherabbild des
//! Prozesses enthielte es im Klartext.

use crate::fehler::Ergebnis;

#[cfg_attr(feature = "ffi", uniffi::export(with_foreign))]
pub trait Tresor: Send + Sync {
    /// `kennung` ist stabil pro Konto und Zweck, etwa `"konto:7:imap"`.
    fn lies(&self, kennung: String) -> Ergebnis<Option<String>>;
    fn schreib(&self, kennung: String, geheimnis: String) -> Ergebnis<()>;
    fn loesche(&self, kennung: String) -> Ergebnis<()>;
}
