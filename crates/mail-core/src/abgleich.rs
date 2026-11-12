//! Kopfzeilen holen, zu Gesprächen zuordnen, speichern.
//!
//! Die Trennung, auf die es ankommt: **Hier** redet jemand mit dem Server,
//! und **nur hier**. Die Gesprächsliste liest ausschliesslich aus der
//! Datenbank. Eine Liste, die selbst ins Netz greift, hat nichts zu zeigen,
//! wenn das Netz fehlt — und sie zeigt beim Blättern Lücken, sobald jemand
//! anderes im Postfach etwas verschiebt.

use mail_proto::{Flagge, Kopfzeilen};
use mail_store::{Nachrichtenzeile, Speicher};
use mail_sync::threading::{self, BekannteGespraeche, Eingang};

/// Wie weit der Betreff-Rückfall zurückschauen darf: vier Wochen.
///
/// Ohne Grenze hängt eine Antwort mit dem Betreff „Angebot" an dem Vorgang,
/// der zufällig als Erstes so hiess — auch wenn der zwei Jahre alt ist.
const BETREFF_FENSTER_MS: i64 = 28 * 24 * 60 * 60 * 1000;

/// Was die Datenbank schon weiss. Der eigene Stapel muss hier NICHT
/// nachgehalten werden — `gruppieren` rechnet den Stapel als Ganzes und
/// kennt seine Mitglieder selbst.
struct Gedaechtnis<'a> {
    speicher: &'a Speicher,
    fenster_ab: i64,
}

impl BekannteGespraeche for Gedaechtnis<'_> {
    fn nach_message_id(&self, message_id: &str) -> Option<String> {
        self.speicher.thread_nach_message_id(message_id).ok().flatten()
    }

    fn nach_betreff(&self, normalisierter_betreff: &str) -> Option<String> {
        self.speicher
            .thread_nach_betreff(normalisierter_betreff, self.fenster_ab)
            .ok()
            .flatten()
    }
}

/// Was der Abgleich verändert hat. Noch liest es niemand — der Rückgabewert
/// steht hier, damit der Aufrufer es melden KANN, sobald die Oberfläche eine
/// Stelle dafür hat.
#[allow(dead_code)]
pub struct Ergebnis {
    pub geschrieben: usize,
    pub zusammengelegt: usize,
}

/// Ordnet einen ganzen Stapel Kopfzeilen zu und schreibt ihn.
///
/// Der Stapel wird als Ganzes gerechnet, nicht Nachricht für Nachricht. Der
/// Unterschied ist nicht akademisch: Eine Antwort und ihre Ausgangsmail
/// tragen oft dieselbe Sekunde, und wer einzeln verarbeitet, hängt vom Zufall
/// der Reihenfolge ab. Genau daran ist die erste Fassung gescheitert — aus
/// drei Nachrichten wurden drei Gespräche statt zwei.
pub fn einordnen(
    speicher: &mut Speicher,
    konto_id: &str,
    ordner_id: &str,
    kopfzeilen: Vec<Kopfzeilen>,
    jetzt_ms: i64,
) -> Result<Ergebnis, mail_store::Speicherfehler> {
    let stapel: Vec<Eingang> = kopfzeilen
        .iter()
        .map(|kopf| Eingang {
            message_id: kopf.message_id.clone(),
            in_reply_to: kopf.in_reply_to.clone(),
            references: kopf.references.clone(),
            betreff: kopf.betreff.clone(),
            gesendet_am: kopf.gesendet_am,
        })
        .collect();

    let gruppierung = {
        let gedaechtnis = Gedaechtnis {
            speicher,
            fenster_ab: jetzt_ms - BETREFF_FENSTER_MS,
        };
        threading::gruppieren(&stapel, &gedaechtnis)
    };

    let zeilen: Vec<Nachrichtenzeile> = kopfzeilen
        .iter()
        .zip(&gruppierung.schluessel)
        .map(|(kopf, schluessel)| Nachrichtenzeile {
            // Die Kennung muss stabil sein: Derselbe Abgleich darf dieselbe
            // Nachricht nicht zweimal anlegen, und nach einem Neustart muss
            // sie wieder genauso heissen.
            id: format!("{ordner_id}/{}", kopf.kennung),
            konto_id: konto_id.to_owned(),
            ordner_id: ordner_id.to_owned(),
            server_kennung: kopf.kennung.clone(),
            message_id: kopf.message_id.as_deref().map(threading::saeubere_id),
            thread_schluessel: schluessel.clone(),
            betreff: kopf.betreff.clone(),
            betreff_normalisiert: threading::normalisiere_betreff(&kopf.betreff),
            von_name: None,
            von_adresse: kopf.von.clone(),
            gesendet_am: kopf.gesendet_am,
            groesse: kopf.groesse,
            flaggen: kopf
                .flaggen
                .iter()
                .map(flagge_als_text)
                .collect::<Vec<_>>()
                .join(","),
            gelesen: kopf.flaggen.contains(&Flagge::Gelesen),
            markiert: kopf.flaggen.contains(&Flagge::Markiert),
            // Braucht BODYSTRUCTURE und kommt mit dem Textkörper nach.
            hat_anhang: false,
        })
        .collect();

    speicher.nachrichten_schreiben(&zeilen)?;

    let mut zusammengelegt = 0;
    for (von, nach) in &gruppierung.zusammenlegen {
        zusammengelegt += speicher.threads_zusammenlegen(&[von.clone()], nach)?;
    }

    Ok(Ergebnis { geschrieben: zeilen.len(), zusammengelegt })
}

fn flagge_als_text(flagge: &Flagge) -> String {
    match flagge {
        Flagge::Gelesen => "gelesen".to_owned(),
        Flagge::Markiert => "markiert".to_owned(),
        Flagge::Beantwortet => "beantwortet".to_owned(),
        Flagge::Weitergeleitet => "weitergeleitet".to_owned(),
        Flagge::Entwurf => "entwurf".to_owned(),
        Flagge::Geloescht => "geloescht".to_owned(),
        Flagge::Eigen(name) => name.clone(),
    }
}
