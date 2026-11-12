//! Der Weg vom Kern zur Oberfläche.
//!
//! Die Oberfläche fragt nicht nach Änderungen, sie bekommt sie gesagt. Das ist
//! der Unterschied zwischen einer Liste, die sich beim Herunterziehen
//! aktualisiert, und einer, die beim Eintreffen einer Mail von selbst wächst
//! — und zwischen einem Kern, der still ist, und einem, der jede Sekunde
//! dreimal gefragt wird.

use crate::ansicht::{Gesundheit, ThreadZeile};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Ereignis {
    /// Ein Abgleich läuft. Die Oberfläche zeigt einen Streifen, keine Sperre.
    AbgleichLaeuft { konto_id: String, ordner_id: String },
    AbgleichFertig { konto_id: String, ordner_id: String, neue: u32 },
    /// Zeilen haben sich geändert — neue Mail, gelesen, markiert, verschoben.
    /// Die Oberfläche tauscht genau diese Zeilen, nicht die Liste.
    ZeilenGeaendert { ordner_id: String, zeilen: Vec<ThreadZeile> },
    ZeilenWeg { ordner_id: String, thread_ids: Vec<String> },
    KontoZustand { konto_id: String, gesundheit: Gesundheit },
    /// Etwas ist schiefgegangen, aber nicht als Antwort auf einen Befehl —
    /// ein Abgleich im Hintergrund etwa. Gehört in eine stille Leiste, nicht
    /// in einen Dialog.
    Stoerung { text: String },
}

/// Von der Plattform implementiert: Swift-Klasse, Kotlin-Objekt, oder auf dem
/// Desktop ein Kanal in die GPUI-Schleife.
#[cfg_attr(feature = "ffi", uniffi::export(with_foreign))]
pub trait EreignisHoerer: Send + Sync {
    fn hoere(&self, ereignis: Ereignis);
}
