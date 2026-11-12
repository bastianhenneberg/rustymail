//! Signaturen und Verschlüsselung. Noch leer.
//!
//! **Die Entscheidung, die hier schon getroffen ist:** `sequoia-openpgp` mit
//! der RustCrypto-Rückseite (`crypto-rust`), nicht mit Nettle. Nettle zieht
//! GMP als C-Bibliothek nach, und die muss dann für aarch64-apple-ios,
//! aarch64-apple-ios-sim, aarch64-linux-android und x86_64-linux-android
//! kreuzkompiliert werden. Das ist der Punkt, an dem Mobile-Builds reißen —
//! und es fällt erst auf, wenn der Kern längst gegen Nettle gebaut ist.
//!
//! S/MIME ist der wichtigere der beiden Wege: In der geschäftlichen Post
//! kommt es häufiger vor als OpenPGP.

#[derive(Debug, thiserror::Error)]
pub enum Kryptofehler {
    #[error("noch nicht gebaut: {0}")]
    NochNichtGebaut(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pruefergebnis {
    /// Gültig signiert, und der Schlüssel gehört zum Absender.
    Gueltig { unterzeichner: String },
    /// Signatur rechnerisch gültig, aber der Schlüssel ist nicht vertrauenswürdig.
    UnbekannterSchluessel { unterzeichner: String },
    Ungueltig { grund: String },
    /// Keine Signatur vorhanden — der Normalfall, und kein Befund.
    Keine,
}
