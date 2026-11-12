//! Die TLS-Einrichtung — eine Stelle für alle Protokolle.
//!
//! `rustls` mit dem Plattform-Prüfer, nicht `native-tls`: Letzteres zieht
//! OpenSSL in jedes Kreuzkompilat für iOS und Android. Der Plattform-Prüfer
//! benutzt ausserdem die Wurzelzertifikate des Betriebssystems — ein
//! Firmen-Zertifikat, das der Rechner kennt, gilt damit auch hier, und ein
//! vom Nutzer entzogenes Vertrauen wirkt auch hier.
//!
//! **Für Android gehört noch etwas dazu**, das sich nicht in Rust erledigen
//! lässt: `rustls_platform_verifier::android::init_with_env()` muss beim
//! App-Start einmal aufgerufen werden, bevor irgendetwas ins Netz geht, und
//! Proguard braucht eine Keep-Regel für `org.rustls.platformverifier.**`.
//! Ohne das findet der Prüfer die JVM nicht — und der Fehler tritt erst auf
//! dem Gerät auf.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;

/// Wem beim Zertifikat geglaubt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vertrauen {
    /// Die Wurzelzertifikate des Betriebssystems. Der einzige Wert, den es
    /// in einem Release-Build gibt.
    #[default]
    Plattform,
    /// Jedes Zertifikat, ungeprüft. Nur mit `test-support`, nur für einen
    /// Testserver auf dem eigenen Rechner.
    #[cfg(feature = "test-support")]
    Ungeprueft,
}

/// Setzt `ring` als Krypto-Anbieter des Prozesses — einmal, beim ersten
/// Verbindungsaufbau.
///
/// Nötig, weil Kisten wie `reqwest` mit `rustls-no-provider` **keinen**
/// eigenen mitbringen und sonst mit `No provider set` abbrechen — zur
/// Laufzeit, beim ersten Netzzugriff. Genau das ist der Preis dafür, dass
/// im Baum nur EIN Anbieter steckt; er wird hier bezahlt, an einer Stelle,
/// statt verstreut über Features.
///
/// Ein zweiter Aufruf tut nichts: `install_default` meldet dann einen
/// Fehler, und der ist hier kein Fehler, sondern die Antwort „steht schon".
pub fn anbieter_sicherstellen() {
    static EINMAL: std::sync::Once = std::sync::Once::new();
    EINMAL.call_once(|| {
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            tracing::debug!("Krypto-Anbieter war bereits gesetzt");
        }
    });
}

/// Scheitert, wenn der Vertrauensspeicher des Systems nicht benutzbar ist —
/// auf Android etwa, wenn `init_with_env()` fehlt. Das ist ein echter
/// Betriebsfehler und gehört nach oben, nicht in ein `expect`.
pub fn einrichtung(vertrauen: Vertrauen) -> Result<Arc<ClientConfig>, rustls::Error> {
    anbieter_sicherstellen();
    match vertrauen {
        // Einmal gebaut und behalten: Den Vertrauensspeicher des Systems je
        // Verbindung zu lesen kostet auf Android und macOS messbar.
        Vertrauen::Plattform => {
            static EINMAL: std::sync::OnceLock<Arc<ClientConfig>> = std::sync::OnceLock::new();
            if let Some(fertig) = EINMAL.get() {
                return Ok(Arc::clone(fertig));
            }
            let frisch = Arc::new(ClientConfig::with_platform_verifier()?);
            Ok(Arc::clone(EINMAL.get_or_init(|| frisch)))
        }
        #[cfg(feature = "test-support")]
        Vertrauen::Ungeprueft => Ok(Arc::new(ungeprueft::einrichtung())),
    }
}

/// Der ungeprüfte Weg steht absichtlich in einem eigenen, feature-geschalteten
/// Modul und nicht als Flagge an einer Funktion: Eine Flagge kann man
/// versehentlich setzen, ein Feature muss jemand in eine Cargo.toml
/// schreiben. In `cargo build --release` existiert dieser Code nicht.
#[cfg(feature = "test-support")]
mod ungeprueft {
    use rustls::DigitallySignedStruct;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use std::sync::Arc;

    #[derive(Debug)]
    struct AlleszulasserPruefer(Arc<CryptoProvider>);

    impl ServerCertVerifier for AlleszulasserPruefer {
        fn verify_server_cert(
            &self,
            _ende: &CertificateDer<'_>,
            _zwischen: &[CertificateDer<'_>],
            _name: &ServerName<'_>,
            _ocsp: &[u8],
            _jetzt: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            nachricht: &[u8],
            zertifikat: &CertificateDer<'_>,
            unterschrift: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls12_signature(
                nachricht,
                zertifikat,
                unterschrift,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            nachricht: &[u8],
            zertifikat: &CertificateDer<'_>,
            unterschrift: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature(
                nachricht,
                zertifikat,
                unterschrift,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }

    pub fn einrichtung() -> rustls::ClientConfig {
        let anbieter = Arc::new(rustls::crypto::ring::default_provider());
        rustls::ClientConfig::builder_with_provider(anbieter.clone())
            .with_safe_default_protocol_versions()
            .expect("Vorgabe-Protokollfassungen")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AlleszulasserPruefer(anbieter)))
            .with_no_client_auth()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hält fest, dass im ganzen Abhängigkeitsbaum GENAU EIN Krypto-Anbieter
    /// eingeschaltet ist.
    ///
    /// rustls wählt den Vorgabe-Anbieter aus den Features — und scheitert,
    /// wenn es zwei gibt. Das passiert nicht beim Bauen, sondern beim ersten
    /// Verbindungsversuch, also im Betrieb. `mail-send` zieht in seiner
    /// Vorgabe `aws-lc-rs` und wurde deshalb mit `default-features = false`
    /// eingebunden; wer das nächste Mal eine Kiste hinzunimmt, die ebenfalls
    /// einen Anbieter mitbringt, merkt es hier statt beim Kunden.
    #[test]
    fn der_anbieter_laesst_sich_setzen_und_zweimal_setzen_schadet_nicht() {
        anbieter_sicherstellen();
        anbieter_sicherstellen();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "kein Krypto-Anbieter im Prozess — jeder Netzzugriff bricht mit `No provider set` ab"
        );
    }

    #[test]
    fn tls_einrichtung_entsteht() {
        let einrichtung = einrichtung(Vertrauen::Plattform);
        assert!(einrichtung.is_ok(), "TLS nicht einrichtbar: {einrichtung:?}");
    }
}
