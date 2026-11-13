//! SASL XOAUTH2 — die Anmeldung, die Gmail und Outlook verlangen.
//!
//! Das Format steht in keinem RFC, sondern in der Dokumentation von Google
//! und Microsoft, und beide verlangen es zeichengenau:
//!
//! ```text
//! user=<adresse>^Aauth=Bearer <token>^A^A
//! ```
//!
//! `^A` ist das Steuerzeichen 0x01, nicht die Zeichenfolge „^A". Beide
//! Trennzeichen am Ende gehören dazu — mit nur einem lehnt Google ab, und
//! die Fehlermeldung nennt den Grund nicht.
//!
//! Der Client schickt diese Zeichenkette base64-kodiert als Antwort auf die
//! leere Aufforderung des Servers. `async-imap` kodiert selbst; hier werden
//! also rohe Bytes zurückgegeben.

/// Die Antwort auf die XOAUTH2-Aufforderung.
pub struct XOauth2 {
    benutzer: String,
    token: String,
}

impl XOauth2 {
    pub fn neu(benutzer: impl Into<String>, token: impl Into<String>) -> Self {
        Self { benutzer: benutzer.into(), token: token.into() }
    }

    /// Die Zeichenkette, die gesendet wird — ohne Base64.
    pub fn antwort(&self) -> Vec<u8> {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.benutzer, self.token).into_bytes()
    }
}

impl async_imap::Authenticator for XOauth2 {
    type Response = Vec<u8>;

    fn process(&mut self, _aufforderung: &[u8]) -> Self::Response {
        // Die Aufforderung ist bei XOAUTH2 leer. Bei einem FEHLER schickt
        // der Server stattdessen einen JSON-Block mit dem Grund und erwartet
        // eine leere Antwort — darum wird der Inhalt hier nicht geprüft,
        // sondern schlicht die Anmeldung geschickt; der Server entscheidet.
        self.antwort()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn das_format_stimmt_zeichengenau() {
        let antwort = XOauth2::neu("post@beispiel.de", "ya29.TOKEN").antwort();
        let text = String::from_utf8(antwort).expect("UTF-8");

        assert_eq!(text, "user=post@beispiel.de\u{1}auth=Bearer ya29.TOKEN\u{1}\u{1}");
        // Die beiden Trennzeichen am Ende sind keine Kosmetik: Mit nur
        // einem lehnt Google ab, ohne den Grund zu nennen.
        assert!(text.ends_with('\u{1}'), "Abschluss fehlt");
        assert_eq!(text.matches('\u{1}').count(), 3, "falsche Anzahl Trennzeichen");
    }

    #[test]
    fn es_wird_kein_caret_a_geschrieben() {
        // Der Fehler, den die Dokumentation provoziert: Dort steht „^A",
        // gemeint ist das Byte 0x01. Wer die Zeichen abtippt, baut eine
        // Anmeldung, die überall abgelehnt wird.
        let antwort = XOauth2::neu("a@b.de", "T").antwort();
        assert!(!antwort.windows(2).any(|paar| paar == b"^A"), "literal ^A geschrieben");
        assert!(antwort.contains(&0x01));
    }

    #[test]
    fn ein_leeres_token_ergibt_trotzdem_gueltige_form() {
        // Kommt vor, wenn der Tresor nichts hergibt. Die Form muss stimmen,
        // damit der Server mit einer klaren Ablehnung antwortet statt mit
        // einem Protokollfehler.
        let text = String::from_utf8(XOauth2::neu("a@b.de", "").antwort()).expect("UTF-8");
        assert_eq!(text, "user=a@b.de\u{1}auth=Bearer \u{1}\u{1}");
    }
}
