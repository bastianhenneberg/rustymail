//! Die Abgleich-Maschine.
//!
//! Hier liegt der Teil, der sich nicht nachträglich einbauen lässt. Ein
//! Client, der beim Start einfach „die letzten 50 Nachrichten" holt,
//! funktioniert in der Vorführung und fällt im Betrieb auf:
//!
//! - Ein Ordner mit 40.000 Nachrichten wird bei jedem Start neu gelesen.
//! - Eine Flagge, die am Telefon gesetzt wurde, kommt am Rechner nie an.
//! - Der Server wird gewechselt, UIDVALIDITY ändert sich, und von da an
//!   zeigt die App Nachrichten, die es nicht gibt.
//!
//! Dagegen stehen drei Dinge, und die gehören von Anfang an hinein:
//! `uidvalidity` je Ordner, `highestmodseq` für die Flaggen (CONDSTORE), und
//! eine Warteschlange für alles, was lokal passiert ist.

use serde::{Deserialize, Serialize};

pub mod threading;

/// Was wir über den Stand eines Ordners wissen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ordnerzustand {
    pub ordner_id: String,
    /// Ändert sich dieser Wert, ist JEDE gespeicherte UID dieses Ordners
    /// wertlos. Das ist kein Sonderfall: Serverumzüge, Wiederherstellungen
    /// aus einer Sicherung und manche Anbieter beim Tarifwechsel lösen es
    /// aus.
    pub uidvalidity: Option<u32>,
    /// Mit CONDSTORE lässt sich nach „alles, was sich seit X geändert hat"
    /// fragen, statt den Ordner durchzuzählen.
    pub highestmodseq: Option<u64>,
    pub hoechste_uid: Option<u32>,
    pub abgeglichen_am: Option<i64>,
}

/// Was der Abgleich zu tun beschlossen hat. Als eigener Typ, damit die
/// Entscheidung prüfbar ist, ohne einen Server zu brauchen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vorgehen {
    /// Nichts zu tun.
    Nichts,
    /// Nur die Neuen holen, ab dieser UID.
    AbUid(u32),
    /// Nur die Änderungen holen (CHANGEDSINCE).
    SeitModseq(u64),
    /// Alles verwerfen und neu lesen. UIDVALIDITY hat sich geändert — die
    /// einzige Antwort darauf, die keine Geister hinterlässt.
    VonVorn { grund: String },
}

pub fn vorgehen_bestimmen(lokal: &Ordnerzustand, server: &mail_proto::Ordner) -> Vorgehen {
    match (lokal.uidvalidity, server.uidvalidity) {
        (Some(alt), Some(neu)) if alt != neu => Vorgehen::VonVorn {
            grund: format!("UIDVALIDITY {alt} → {neu}"),
        },
        (None, _) => Vorgehen::VonVorn {
            grund: "noch nie abgeglichen".to_owned(),
        },
        _ => match (lokal.highestmodseq, server.highestmodseq) {
            (Some(alt), Some(neu)) if neu > alt => Vorgehen::SeitModseq(alt),
            (Some(alt), Some(neu)) if neu == alt => Vorgehen::Nichts,
            _ => match lokal.hoechste_uid {
                Some(uid) => Vorgehen::AbUid(uid + 1),
                None => Vorgehen::VonVorn {
                    grund: "kein Stand".to_owned(),
                },
            },
        },
    }
}

/// Eine lokale Änderung, die noch zum Server muss.
///
/// Jede Art muss **idempotent wiederholbar** sein. Das Netz bricht mitten im
/// Senden weg, und beim nächsten Start weiß niemand, ob der Server die
/// Operation gesehen hat. „Als gelesen markieren" zweimal ist harmlos;
/// „verschieben" zweimal nicht — deshalb trägt `Verschieben` die Zielkennung
/// und nicht „einen Ordner weiter".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AusstehendeOperation {
    FlaggeSetzen { nachricht_id: String, flagge: String, gesetzt: bool },
    Verschieben { nachricht_id: String, nach_ordner_id: String },
    Senden { entwurf_id: String },
    /// Die Kopie in „Gesendet" ablegen. Eigene Operation, weil sie auch dann
    /// nachgeholt werden muss, wenn das Senden längst geklappt hat — sonst
    /// fehlt die Kopie still.
    InGesendetAblegen { entwurf_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_ordner(uidvalidity: Option<u32>, modseq: Option<u64>) -> mail_proto::Ordner {
        mail_proto::Ordner {
            pfad: "INBOX".into(),
            name: "INBOX".into(),
            trenner: '/',
            gesamt: 0,
            ungelesen: 0,
            rolle: None,
            uidvalidity,
            highestmodseq: modseq,
        }
    }

    fn lokal(uidvalidity: Option<u32>, modseq: Option<u64>, uid: Option<u32>) -> Ordnerzustand {
        Ordnerzustand {
            ordner_id: "o1".into(),
            uidvalidity,
            highestmodseq: modseq,
            hoechste_uid: uid,
            abgeglichen_am: Some(0),
        }
    }

    #[test]
    fn geaenderte_uidvalidity_erzwingt_neulesen() {
        let vorgehen = vorgehen_bestimmen(
            &lokal(Some(1), Some(10), Some(500)),
            &server_ordner(Some(2), Some(10)),
        );
        assert!(matches!(vorgehen, Vorgehen::VonVorn { .. }));
    }

    #[test]
    fn gleicher_modseq_heisst_nichts_zu_tun() {
        let vorgehen = vorgehen_bestimmen(
            &lokal(Some(1), Some(10), Some(500)),
            &server_ordner(Some(1), Some(10)),
        );
        assert_eq!(vorgehen, Vorgehen::Nichts);
    }

    #[test]
    fn ohne_condstore_wird_ab_der_letzten_uid_geholt() {
        let vorgehen = vorgehen_bestimmen(
            &lokal(Some(1), None, Some(500)),
            &server_ordner(Some(1), None),
        );
        assert_eq!(vorgehen, Vorgehen::AbUid(501));
    }
}
