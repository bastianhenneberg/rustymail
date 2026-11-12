//! Der Kern des Mail-Clients: alles außer der Darstellung.
//!
//! Drei Oberflächen hängen daran — die Desktop-App (linkt diese Kiste
//! direkt), SwiftUI auf iOS und Compose auf Android (über `bindings/mail-ffi`).
//! Was hier liegt, muss auf allen drei gleich sein; was sich pro Plattform
//! unterscheidet, gehört in eine Hülle: Zugangsdaten (`Tresor`), die
//! Darstellung des Textkörpers, Benachrichtigungen, Dateiauswahl.
//!
//! Diese App steht für sich. Sie hat nichts mit `peppermint/mailbox` zu tun —
//! das Paket bleibt, wo es ist, und bedient die Online-Produkte weiter. Hier
//! gibt es keine Mitte, keine Zuweisung und keine Werkzeugleiste aus einem
//! fremden System.
//!
//! Diese Kiste baut ohne NDK, ohne Xcode und ohne JVM. Das ist Absicht und
//! soll so bleiben: Es ist der Unterschied zwischen „ich kann die Logik
//! prüfen" und „ich erfahre nach 15 Minuten Cloud-Build, ob sie stimmt".

mod abgleich;
pub mod antwort;
pub mod ansicht;
pub mod dokument;
pub mod ereignis;
pub mod fehler;
pub mod kern;
pub mod koerper;
pub mod konto;
pub mod tresor;

pub use ansicht::*;
pub use ereignis::{Ereignis, EreignisHoerer};
pub use fehler::{Ergebnis, Fehler};
pub use kern::{Einrichtung, Kern};
pub use konto::{Kontoentwurf, Protokoll, Verbindungsdaten, Verschluesselung};
pub use tresor::Tresor;

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();
