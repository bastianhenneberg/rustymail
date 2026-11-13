//! Rein lesende Probe gegen den echten Stalwart.
//!
//! Gibt KEINE Inhalte aus — nur Zahlen. Was in diesem Postfach steht, ist
//! Kundenpost und gehoert nicht in ein Protokoll.

use mail_proto::imap::ImapBackend;
use mail_proto::{Anmeldeart, Kontobackend, Verschluesselung, Zugang};

#[tokio::main]
async fn main() {
    let zugang = Zugang {
        host: std::env::var("PROBE_HOST").unwrap(),
        port: 993,
        benutzer: std::env::var("PROBE_USER").unwrap(),
        geheimnis: std::env::var("PROBE_PASS").unwrap(),
        verschluesselung: Verschluesselung::Tls,
        anmeldeart: Anmeldeart::Passwort,
        senden: None,
    };

    // KEIN neu_ungeprueft: Das echte Let's-Encrypt-Zertifikat soll gegen den
    // Vertrauensspeicher des Systems geprueft werden. Genau das ist hier mit
    // zu beweisen.
    let backend = ImapBackend::neu(zugang);

    match backend.ordner().await {
        Ok(ordner) => {
            println!("Ordner: {}", ordner.len());
            for o in &ordner {
                println!("  {} (Rolle {:?})", o.name, o.rolle);
            }

            let eingang = ordner
                .iter()
                .find(|o| o.pfad.eq_ignore_ascii_case("INBOX"))
                .expect("INBOX");

            match backend.kopfzeilen(&eingang.pfad, None, 5).await {
                Ok(seite) => {
                    println!("Kopfzeilen geholt: {}", seite.kopfzeilen.len());
                    let mit_betreff = seite.kopfzeilen.iter().filter(|k| !k.betreff.is_empty()).count();
                    let mit_datum = seite.kopfzeilen.iter().filter(|k| k.gesendet_am > 0).count();
                    let mit_absender = seite.kopfzeilen.iter().filter(|k| !k.von.is_empty()).count();
                    let gelesen = seite.kopfzeilen.iter().filter(|k| k.flaggen.contains(&mail_proto::Flagge::Gelesen)).count();
                    println!("  mit Betreff: {mit_betreff}, mit Datum: {mit_datum}, mit Absender: {mit_absender}, davon gelesen: {gelesen}");
                    println!("  Umlaute im Betreff dekodiert: {}", 
                        seite.kopfzeilen.iter().all(|k| !k.betreff.contains("=?")));
                }
                Err(f) => println!("Kopfzeilen: FEHLER {f}"),
            }
        }
        Err(f) => println!("Ordner: FEHLER {f}"),
    }
}
