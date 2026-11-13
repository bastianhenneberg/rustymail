//! Die Desktop-Oberfläche: Linux, macOS und Windows aus einem Binary.
//!
//! Selbst gezeichnet, ohne WebView. Das betrifft auch den Mailtext: Was an
//! HTML in einer Nachricht steckt, zeichnet diese App selbst — Aufgabe #9869
//! entscheidet, wie weit. Ein Browser im Fenster kommt hier nicht hinein,
//! und zwar nicht nur aus Geschmack: Er brächte eine Skript-Laufzeit und
//! einen eigenen Netzzugang in die Anzeige fremder Post.
//!
//! Die Oberfläche hält keinen Zustand, den der Kern nicht kennt. Sie fragt,
//! stellt dar, und schickt Befehle — nichts, was sie hier entscheidet, darf
//! auf iOS oder Android anders entschieden werden.

use std::sync::Arc;

use iced::font::{Style as Schriftstil, Weight as Schriftstaerke};
use iced::widget::{
    button, column, container, rich_text, row, scrollable, span, text, text_editor, text_input,
};
use iced::{Font, Padding};
use iced::{Element, Length, Task, Theme};

use mail_core::dokument::{Abschnitt, Block, Dokument};
use mail_core::{
    Einrichtung, Ereignis, EreignisHoerer, Kern, Kontoentwurf, KontoAnsicht, OrdnerAnsicht,
    NachrichtAnsicht, Ordnerrolle, Protokoll, ThreadZeile, Tresor, Verbindungsdaten,
    Verschluesselung,
};

const DIENST: &str = "rustymail";

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application(starten, aktualisieren, ansicht)
        .window(iced::window::Settings {
            // Die Fensterkennung muss zum Starter-Eintrag passen
            // (`StartupWMClass=rustymail`). Ohne sie ordnet der Compositor
            // das Fenster keinem Programm zu: kein Symbol in der Leiste,
            // und ein zweiter Start öffnet scheinbar etwas Fremdes.
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "rustymail".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        })
        .title("RustyMail")
        .theme(|_zustand: &Postfach| Theme::Dark)
        .window_size((1280.0, 800.0))
        .run()
}

/// Zugangsdaten auf dem Desktop: Apple Keychain, Windows Credential Manager,
/// oder der Secret Service über D-Bus. Welcher es ist, entscheidet `keyring`
/// anhand des Betriebssystems — hier steht kein `cfg`.
struct Schluesselbund;

impl Tresor for Schluesselbund {
    fn lies(&self, kennung: String) -> mail_core::Ergebnis<Option<String>> {
        match eintrag(&kennung)?.get_password() {
            Ok(geheimnis) => Ok(Some(geheimnis)),
            // „Nicht da" ist kein Fehler: Beim ersten Start ist nichts da.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(fehler) => Err(mail_core::Fehler::Speicher {
                grund: format!("Tresor nicht lesbar: {fehler}"),
            }),
        }
    }

    fn schreib(&self, kennung: String, geheimnis: String) -> mail_core::Ergebnis<()> {
        eintrag(&kennung)?
            .set_password(&geheimnis)
            .map_err(|fehler| mail_core::Fehler::Speicher {
                grund: format!("Tresor nicht beschreibbar: {fehler}"),
            })
    }

    fn loesche(&self, kennung: String) -> mail_core::Ergebnis<()> {
        match eintrag(&kennung)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(fehler) => Err(mail_core::Fehler::Speicher {
                grund: format!("Tresoreintrag nicht löschbar: {fehler}"),
            }),
        }
    }
}

fn eintrag(kennung: &str) -> mail_core::Ergebnis<keyring::Entry> {
    keyring::Entry::new(DIENST, kennung).map_err(|fehler| mail_core::Fehler::Speicher {
        grund: format!("kein Tresor erreichbar: {fehler}"),
    })
}

/// Ereignisse aus dem Kern. Noch nur ins Protokoll: Der Rückruf kommt aus
/// einem fremden Thread, und Iced will seine Änderungen als Nachricht in der
/// eigenen Schleife. Beim Anbinden wird hier ein Kanal daraus, den eine
/// `Subscription` ausliest — nicht ein Zugriff auf den Zustand von aussen.
struct Protokollhoerer;

impl EreignisHoerer for Protokollhoerer {
    fn hoere(&self, ereignis: Ereignis) {
        tracing::debug!(?ereignis, "Kern meldet");
    }
}

#[derive(Debug, Clone)]
enum Nachricht {
    KontenGeladen(Result<Vec<KontoAnsicht>, String>),
    KontoGewaehlt(String),
    OrdnerGeladen(Result<Vec<OrdnerAnsicht>, String>),
    OrdnerGewaehlt(String),
    Abgeglichen(Result<String, String>),
    ThreadsGeladen(Result<Vec<ThreadZeile>, String>),
    GespraechGewaehlt(String),
    GespraechGeladen(Result<Vec<NachrichtAnsicht>, String>),
    ZurueckZumKonto,
    AntwortBearbeitet(text_editor::Action),
    AntwortSenden,
    AntwortGesendet(Result<(), String>),
    GelesenSetzen(bool),
    VerweisGeklickt(String),
    Markieren(bool),
    FlaggeGesetzt(Result<(), String>),
    Archivieren,
    InDenPapierkorb,
    Verschoben(Result<(), String>),
    Feld(Feld, String),
    Anlegen,
    Angelegt(Result<String, String>),
    Entfernen(String),
    Entfernt(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feld {
    Beschriftung,
    Adresse,
    Host,
    Port,
    Benutzer,
    Passwort,
    SmtpHost,
    SmtpPort,
}

#[derive(Default)]
struct Formular {
    beschriftung: String,
    adresse: String,
    host: String,
    port: String,
    benutzer: String,
    passwort: String,
    smtp_host: String,
    smtp_port: String,
}

impl Formular {
    fn zu_entwurf(&self) -> Result<Kontoentwurf, String> {
        let port: u16 = if self.port.trim().is_empty() {
            993
        } else {
            self.port
                .trim()
                .parse()
                .map_err(|_| format!("„{}“ ist kein Port", self.port))?
        };

        if self.adresse.trim().is_empty() {
            return Err("Ohne Adresse geht es nicht.".to_owned());
        }
        if self.host.trim().is_empty() {
            return Err("Ohne Server geht es nicht.".to_owned());
        }

        Ok(Kontoentwurf {
            beschriftung: self.beschriftung.trim().to_owned(),
            adresse: self.adresse.trim().to_owned(),
            protokoll: Protokoll::Imap,
            verbindung: Verbindungsdaten {
                // Im Formular immer Passwort: XOAUTH2 braucht den
                // Browser-Teil der Anmeldung, und den gibt es noch nicht.
                anmeldeart: mail_core::Anmeldeart::Passwort,
                host: self.host.trim().to_owned(),
                // Der Benutzername ist meistens die Adresse — aber nicht
                // immer, deshalb ein eigenes Feld mit Vorbelegung.
                benutzer: if self.benutzer.trim().is_empty() {
                    self.adresse.trim().to_owned()
                } else {
                    self.benutzer.trim().to_owned()
                },
                port,
                verschluesselung: if port == 143 {
                    Verschluesselung::Starttls
                } else {
                    Verschluesselung::Tls
                },
                jmap_sitzung: None,
                smtp_host: (!self.smtp_host.trim().is_empty())
                    .then(|| self.smtp_host.trim().to_owned()),
                smtp_port: self.smtp_port.trim().parse().ok(),
            },
            geheimnis: self.passwort.clone(),
        })
    }
}

struct Postfach {
    kern: Option<Arc<Kern>>,
    konten: Vec<KontoAnsicht>,
    gewaehltes_konto: Option<String>,
    ordner: Vec<OrdnerAnsicht>,
    gewaehlter_ordner: Option<String>,
    threads: Vec<ThreadZeile>,
    gewaehltes_gespraech: Option<String>,
    nachrichten: Vec<NachrichtAnsicht>,
    gespraech_laedt: bool,
    antwort: text_editor::Content,
    antwort_laeuft: bool,
    laedt: bool,
    formular: Formular,
    hinweis: Option<String>,
}

fn starten() -> (Postfach, Task<Nachricht>) {
    let kern = Kern::neu(
        Einrichtung { datenverzeichnis: datenverzeichnis() },
        Arc::new(Schluesselbund),
        Arc::new(Protokollhoerer),
    );

    match kern {
        Ok(kern) => {
            let laden = konten_laden(&kern);
            (
                Postfach {
                    kern: Some(kern),
                    konten: Vec::new(),
                    gewaehltes_konto: None,
                    ordner: Vec::new(),
                    gewaehlter_ordner: None,
                    threads: Vec::new(),
                    gewaehltes_gespraech: None,
                    nachrichten: Vec::new(),
                    gespraech_laedt: false,
                    antwort: text_editor::Content::new(),
                    antwort_laeuft: false,
                    laedt: false,
                    formular: Formular { port: "993".into(), ..Formular::default() },
                    hinweis: None,
                },
                laden,
            )
        }
        // Ohne Kern gibt es keine App — aber ein Fenster mit der Begründung
        // ist besser als ein Absturz beim Start, bei dem niemand die Meldung
        // sieht.
        Err(fehler) => (
            Postfach {
                kern: None,
                konten: Vec::new(),
                gewaehltes_konto: None,
                ordner: Vec::new(),
                gewaehlter_ordner: None,
                threads: Vec::new(),
                gewaehltes_gespraech: None,
                nachrichten: Vec::new(),
                gespraech_laedt: false,
                antwort: text_editor::Content::new(),
                antwort_laeuft: false,
                laedt: false,
                formular: Formular::default(),
                hinweis: Some(fehler.to_string()),
            },
            Task::none(),
        ),
    }
}

fn konten_laden(kern: &Arc<Kern>) -> Task<Nachricht> {
    let kern = kern.clone();
    Task::perform(
        async move { kern.konten().await.map_err(|fehler| fehler.to_string()) },
        Nachricht::KontenGeladen,
    )
}

fn aktualisieren(zustand: &mut Postfach, nachricht: Nachricht) -> Task<Nachricht> {
    match nachricht {
        Nachricht::KontenGeladen(Ok(konten)) => {
            let einziges = (konten.len() == 1 && zustand.gewaehltes_konto.is_none())
                .then(|| konten[0].id.clone());
            zustand.konten = konten;

            match einziges {
                Some(id) => aktualisieren(zustand, Nachricht::KontoGewaehlt(id)),
                None => Task::none(),
            }
        }
        // Fehler werden gezeigt, nicht verschluckt. Eine Oberfläche, die bei
        // einem Fehlschlag leer bleibt, sieht aus wie ein Postfach ohne Post.
        Nachricht::KontenGeladen(Err(grund)) => {
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::KontoGewaehlt(id) => {
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };
            zustand.gewaehltes_konto = Some(id.clone());
            zustand.ordner.clear();
            // Sichtbar machen, DASS etwas läuft. Eine Ordnerliste über eine
            // langsame Leitung braucht Sekunden, und eine Oberfläche, die
            // dabei unverändert aussieht, wirkt kaputt.
            zustand.laedt = true;
            zustand.hinweis = None;
            Task::perform(
                async move { kern.ordner(id).await.map_err(|fehler| fehler.to_string()) },
                Nachricht::OrdnerGeladen,
            )
        }
        Nachricht::OrdnerGeladen(Ok(ordner)) => {
            tracing::info!(anzahl = ordner.len(), "Ordnerliste da");
            // Den Eingang gleich öffnen. Niemand startet ein Mailprogramm,
            // um danach den Eingang anzuklicken.
            let eingang = ordner
                .iter()
                .find(|o| o.rolle == Ordnerrolle::Eingang)
                .map(|o| o.id.clone());
            zustand.ordner = ordner;
            zustand.laedt = false;

            match eingang {
                Some(id) => aktualisieren(zustand, Nachricht::OrdnerGewaehlt(id)),
                None => Task::none(),
            }
        }
        Nachricht::OrdnerGewaehlt(id) => {
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };
            zustand.gewaehlter_ordner = Some(id.clone());
            zustand.threads.clear();
            zustand.laedt = true;
            zustand.hinweis = None;

            // Erst abgleichen, dann lesen. Die Liste liest NUR aus der
            // Datenbank — damit sie auch ohne Netz etwas zu zeigen hat.
            Task::perform(
                async move {
                    kern.abgleichen(id.clone())
                        .await
                        .map(|()| id)
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::Abgeglichen,
            )
        }
        Nachricht::Abgeglichen(Ok(ordner_id)) => {
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };
            Task::perform(
                async move {
                    kern.threads(ordner_id, None, 100)
                        .await
                        .map(|seite| seite.zeilen)
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::ThreadsGeladen,
            )
        }
        // Der Abgleich ist gescheitert — die Liste trotzdem aus der
        // Datenbank zeigen. Was beim letzten Mal ankam, ist mehr als nichts.
        Nachricht::Abgeglichen(Err(grund)) => {
            tracing::warn!(%grund, "Abgleich fehlgeschlagen");
            zustand.hinweis = Some(grund);
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };
            let Some(ordner_id) = zustand.gewaehlter_ordner.clone() else {
                zustand.laedt = false;
                return Task::none();
            };
            Task::perform(
                async move {
                    kern.threads(ordner_id, None, 100)
                        .await
                        .map(|seite| seite.zeilen)
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::ThreadsGeladen,
            )
        }
        Nachricht::ThreadsGeladen(Ok(threads)) => {
            tracing::info!(anzahl = threads.len(), "Gesprächsliste da");
            zustand.threads = threads;
            zustand.laedt = false;
            Task::none()
        }
        Nachricht::GespraechGewaehlt(thread_id) => {
            let (Some(kern), Some(ordner_id)) =
                (zustand.kern.clone(), zustand.gewaehlter_ordner.clone())
            else {
                return Task::none();
            };
            zustand.gewaehltes_gespraech = Some(thread_id.clone());
            zustand.nachrichten.clear();
            zustand.gespraech_laedt = true;
            zustand.hinweis = None;
            tracing::info!(%thread_id, "Gespräch angefragt");
            Task::perform(
                async move {
                    kern.gespraech(ordner_id, thread_id)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::GespraechGeladen,
            )
        }
        Nachricht::GespraechGeladen(Ok(nachrichten)) => {
            tracing::info!(anzahl = nachrichten.len(), "Gespräch da");
            zustand.nachrichten = nachrichten;
            zustand.gespraech_laedt = false;

            // Geöffnet heisst gelesen — aber nur, wenn es vorher ungelesen
            // war. Sonst landet bei jedem Öffnen ein überflüssiger Auftrag
            // in der Warteschlange.
            let war_ungelesen = zustand
                .gewaehltes_gespraech
                .as_deref()
                .and_then(|id| zustand.threads.iter().find(|zeile| zeile.thread_id == id))
                .is_some_and(|zeile| zeile.ungelesen);

            if war_ungelesen {
                aktualisieren(zustand, Nachricht::GelesenSetzen(true))
            } else {
                Task::none()
            }
        }
        Nachricht::GespraechGeladen(Err(grund)) => {
            tracing::warn!(%grund, "Gespräch fehlgeschlagen");
            zustand.gespraech_laedt = false;
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::ZurueckZumKonto => {
            zustand.gewaehltes_gespraech = None;
            zustand.gespraech_laedt = false;
            zustand.nachrichten.clear();
            zustand.antwort = text_editor::Content::new();
            Task::none()
        }
        Nachricht::AntwortBearbeitet(aktion) => {
            zustand.antwort.perform(aktion);
            Task::none()
        }
        Nachricht::AntwortSenden => {
            let (Some(kern), Some(ordner_id), Some(thread_id)) = (
                zustand.kern.clone(),
                zustand.gewaehlter_ordner.clone(),
                zustand.gewaehltes_gespraech.clone(),
            ) else {
                return Task::none();
            };

            let text = zustand.antwort.text();
            if text.trim().is_empty() {
                zustand.hinweis = Some("Die Antwort ist leer.".to_owned());
                return Task::none();
            }

            zustand.antwort_laeuft = true;
            zustand.hinweis = None;
            Task::perform(
                async move {
                    kern.antworten(ordner_id, thread_id, text)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::AntwortGesendet,
            )
        }
        Nachricht::AntwortGesendet(Ok(())) => {
            tracing::info!("Antwort gesendet");
            zustand.antwort_laeuft = false;
            // Das Feld erst NACH dem Erfolg leeren. Wer es vorher leert,
            // verliert den getippten Text, wenn der Server ablehnt.
            zustand.antwort = text_editor::Content::new();

            let (Some(kern), Some(ordner_id), Some(thread_id)) = (
                zustand.kern.clone(),
                zustand.gewaehlter_ordner.clone(),
                zustand.gewaehltes_gespraech.clone(),
            ) else {
                return Task::none();
            };
            Task::perform(
                async move {
                    kern.abgleichen(ordner_id.clone()).await.ok();
                    kern.gespraech(ordner_id, thread_id)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::GespraechGeladen,
            )
        }
        Nachricht::GelesenSetzen(gelesen) => {
            let (Some(kern), Some(ordner_id), Some(thread_id)) = (
                zustand.kern.clone(),
                zustand.gewaehlter_ordner.clone(),
                zustand.gewaehltes_gespraech.clone(),
            ) else {
                return Task::none();
            };
            // Die Liste gleich mitziehen, damit der Punkt sofort umspringt —
            // der Kern hat lokal schon geschrieben.
            for zeile in &mut zustand.threads {
                if zeile.thread_id == thread_id {
                    zeile.ungelesen = !gelesen;
                }
            }
            Task::perform(
                async move {
                    kern.gelesen_setzen(ordner_id, thread_id, gelesen)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::FlaggeGesetzt,
            )
        }
        Nachricht::Markieren(markiert) => {
            let (Some(kern), Some(ordner_id), Some(thread_id)) = (
                zustand.kern.clone(),
                zustand.gewaehlter_ordner.clone(),
                zustand.gewaehltes_gespraech.clone(),
            ) else {
                return Task::none();
            };
            for zeile in &mut zustand.threads {
                if zeile.thread_id == thread_id {
                    zeile.markiert = markiert;
                }
            }
            Task::perform(
                async move {
                    kern.markieren(ordner_id, thread_id, markiert)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::FlaggeGesetzt,
            )
        }
        // Noch kein Öffnen im Browser: Das ist ein Schritt nach draussen,
        // und er gehört ausdrücklich gewollt. Bis dahin wird wenigstens
        // sichtbar, WOHIN ein Verweis führt — bei Post von Fremden ist das
        // die wichtigere Hälfte.
        Nachricht::VerweisGeklickt(ziel) => {
            zustand.hinweis = Some(format!("Verweis führt zu: {ziel}"));
            Task::none()
        }
        Nachricht::Archivieren | Nachricht::InDenPapierkorb => {
            let ins_archiv = matches!(nachricht, Nachricht::Archivieren);
            let (Some(kern), Some(ordner_id), Some(thread_id)) = (
                zustand.kern.clone(),
                zustand.gewaehlter_ordner.clone(),
                zustand.gewaehltes_gespraech.clone(),
            ) else {
                return Task::none();
            };

            // Die Zeile sofort aus der Liste nehmen und zurück zur
            // Übersicht. Der Kern hat lokal schon gelöscht; ein Gespräch,
            // das noch offen steht, nachdem man es archiviert hat, wirkt
            // wie ein nicht angekommener Klick.
            zustand.threads.retain(|zeile| zeile.thread_id != thread_id);
            zustand.gewaehltes_gespraech = None;
            zustand.nachrichten.clear();
            zustand.hinweis = None;

            Task::perform(
                async move {
                    if ins_archiv {
                        kern.archivieren(ordner_id, thread_id).await
                    } else {
                        kern.loeschen(ordner_id, thread_id).await
                    }
                    .map_err(|fehler| fehler.to_string())
                },
                Nachricht::Verschoben,
            )
        }
        Nachricht::Verschoben(Ok(())) => Task::none(),
        Nachricht::Verschoben(Err(grund)) => {
            tracing::warn!(%grund, "Verschieben fehlgeschlagen");
            // Hier wird NICHT zurückgerollt: Lokal ist es weg, der Auftrag
            // liegt in der Warteschlange, und der nächste Abgleich bringt
            // das Gespräch zurück, falls es wirklich nicht ging.
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::FlaggeGesetzt(Ok(())) => Task::none(),
        Nachricht::FlaggeGesetzt(Err(grund)) => {
            tracing::warn!(%grund, "Flagge nicht gesetzt");
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::AntwortGesendet(Err(grund)) => {
            tracing::warn!(%grund, "Antwort fehlgeschlagen");
            zustand.antwort_laeuft = false;
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::ThreadsGeladen(Err(grund)) => {
            tracing::warn!(%grund, "Gesprächsliste fehlgeschlagen");
            zustand.laedt = false;
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::OrdnerGeladen(Err(grund)) => {
            tracing::warn!(%grund, "Ordnerliste fehlgeschlagen");
            zustand.laedt = false;
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::Feld(feld, wert) => {
            match feld {
                Feld::Beschriftung => zustand.formular.beschriftung = wert,
                Feld::Adresse => zustand.formular.adresse = wert,
                Feld::Host => zustand.formular.host = wert,
                Feld::Port => zustand.formular.port = wert,
                Feld::Benutzer => zustand.formular.benutzer = wert,
                Feld::Passwort => zustand.formular.passwort = wert,
                Feld::SmtpHost => zustand.formular.smtp_host = wert,
                Feld::SmtpPort => zustand.formular.smtp_port = wert,
            }
            Task::none()
        }
        Nachricht::Anlegen => {
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };

            match zustand.formular.zu_entwurf() {
                Ok(entwurf) => {
                    zustand.hinweis = None;
                    Task::perform(
                        async move {
                            kern.konto_anlegen(entwurf)
                                .await
                                .map_err(|fehler| fehler.to_string())
                        },
                        Nachricht::Angelegt,
                    )
                }
                Err(grund) => {
                    zustand.hinweis = Some(grund);
                    Task::none()
                }
            }
        }
        Nachricht::Angelegt(Ok(_id)) => {
            // Das Passwort wird aus dem Zustand genommen, sobald es im Tresor
            // liegt. Es im Formular stehen zu lassen, hiesse es für die
            // Laufzeit des Fensters im Speicher zu halten, ohne Grund.
            zustand.formular = Formular { port: "993".into(), ..Formular::default() };
            zustand.hinweis = None;
            match zustand.kern.as_ref() {
                Some(kern) => konten_laden(kern),
                None => Task::none(),
            }
        }
        Nachricht::Angelegt(Err(grund)) => {
            zustand.hinweis = Some(grund);
            Task::none()
        }
        Nachricht::Entfernen(id) => {
            let Some(kern) = zustand.kern.clone() else {
                return Task::none();
            };
            Task::perform(
                async move {
                    kern.konto_entfernen(id)
                        .await
                        .map_err(|fehler| fehler.to_string())
                },
                Nachricht::Entfernt,
            )
        }
        Nachricht::Entfernt(Ok(())) => {
            zustand.gewaehltes_konto = None;
            zustand.ordner.clear();
            match zustand.kern.as_ref() {
                Some(kern) => konten_laden(kern),
                None => Task::none(),
            }
        }
        Nachricht::Entfernt(Err(grund)) => {
            zustand.hinweis = Some(grund);
            Task::none()
        }
    }
}

fn ansicht(zustand: &Postfach) -> Element<'_, Nachricht> {
    row![
        spalte_konten(zustand),
        spalte_gespraeche(zustand),
        spalte_rechts(zustand),
    ]
    .into()
}

fn spalte_konten(zustand: &Postfach) -> Element<'_, Nachricht> {
    let mut inhalt = column![text("Postfächer").size(13)].spacing(10).padding(16);

    if zustand.konten.is_empty() {
        inhalt = inhalt.push(text("noch keins").size(12));
    }

    for konto in &zustand.konten {
        let gewaehlt = zustand.gewaehltes_konto.as_deref() == Some(konto.id.as_str());

        inhalt = inhalt.push(
            column![
                button(text(konto.beschriftung.clone()).size(14))
                    .on_press(Nachricht::KontoGewaehlt(konto.id.clone())),
                text(konto.adresse.clone()).size(11),
            ]
            .spacing(2),
        );

        if gewaehlt {
            if zustand.laedt {
                inhalt = inhalt.push(text("  lädt…").size(11));
            }
            for ordner in &zustand.ordner {
                // Systemordner erkennbar machen, ohne auf den Namen zu
                // schauen — der heisst je Anbieter und Sprache anders.
                let zeichen = match ordner.rolle {
                    Ordnerrolle::Eingang => "◆",
                    Ordnerrolle::Gesendet => "↗",
                    Ordnerrolle::Entwuerfe => "✎",
                    Ordnerrolle::Papierkorb => "␡",
                    Ordnerrolle::Spam => "⚠",
                    Ordnerrolle::Archiv => "▣",
                    Ordnerrolle::Sonstiger => "·",
                };
                inhalt = inhalt.push(
                    button(text(format!("{zeichen} {}", ordner.beschriftung)).size(12))
                        .on_press(Nachricht::OrdnerGewaehlt(ordner.id.clone())),
                );
            }
            inhalt = inhalt.push(
                button(text("Konto entfernen").size(11))
                    .on_press(Nachricht::Entfernen(konto.id.clone())),
            );
        }
    }

    container(scrollable(inhalt))
        .width(Length::Fixed(260.0))
        .height(Length::Fill)
        .style(trennlinie_rechts)
        .into()
}

/// Die Gesprächsliste. Hier entscheidet sich später, ob die App schnell ist:
/// Ein Ordner mit 40.000 Nachrichten darf keine 40.000 Zeilen bauen, sondern
/// nur die sichtbaren.
fn spalte_gespraeche(zustand: &Postfach) -> Element<'_, Nachricht> {
    let mut inhalt = column![text("Gespräche").size(13)].spacing(10).padding(16);

    if zustand.gewaehlter_ordner.is_none() {
        inhalt = inhalt.push(text("kein Ordner gewählt").size(12));
    } else if zustand.laedt {
        inhalt = inhalt.push(text("lädt…").size(12));
    } else if zustand.threads.is_empty() {
        inhalt = inhalt.push(text("keine Nachrichten").size(12));
    }

    for thread in &zustand.threads {
        // Ungelesen wird durch ein Zeichen gezeigt, nicht nur durch
        // Fettdruck: Fettdruck allein ist für manche Leute kein Unterschied,
        // und bei Sonnenlicht auf dem Telefon für niemanden.
        let marke = if thread.ungelesen { "●" } else { "○" };
        let anzahl = if thread.anzahl > 1 {
            format!(" ({})", thread.anzahl)
        } else {
            String::new()
        };

        inhalt = inhalt.push(
            button(
                column![
                    text(format!("{marke} {}{anzahl}", thread.betreff)).size(13),
                    text(format!(
                        "{} · {}",
                        thread.beteiligte.join(", "),
                        zeitpunkt(thread.letzte_nachricht_am)
                    ))
                    .size(11),
                ]
                .spacing(1),
            )
            .width(Length::Fill)
            .on_press(Nachricht::GespraechGewaehlt(thread.thread_id.clone())),
        );
    }

    container(scrollable(inhalt))
        .width(Length::Fixed(380.0))
        .height(Length::Fill)
        .style(trennlinie_rechts)
        .into()
}

/// Die geöffnete Post. Nachrichten untereinander, älteste zuerst — ein
/// Verlauf liest sich von oben nach unten.
fn spalte_gespraech(zustand: &Postfach) -> Element<'_, Nachricht> {
    let aktuelle_zeile = zustand
        .gewaehltes_gespraech
        .as_deref()
        .and_then(|id| zustand.threads.iter().find(|zeile| zeile.thread_id == id));
    let ist_markiert = aktuelle_zeile.is_some_and(|zeile| zeile.markiert);
    let ist_ungelesen = aktuelle_zeile.is_some_and(|zeile| zeile.ungelesen);

    let mut inhalt = column![
        row![
            button(text("← zurück").size(11)).on_press(Nachricht::ZurueckZumKonto),
            button(text(if ist_markiert { "★ markiert" } else { "☆ markieren" }).size(11))
                .on_press(Nachricht::Markieren(!ist_markiert)),
            button(
                text(if ist_ungelesen { "als gelesen" } else { "als ungelesen" }).size(11)
            )
            .on_press(Nachricht::GelesenSetzen(ist_ungelesen)),
            button(text("▣ archivieren").size(11)).on_press(Nachricht::Archivieren),
            button(text("␡ Papierkorb").size(11)).on_press(Nachricht::InDenPapierkorb),
        ]
        .spacing(6)
    ]
    .spacing(14)
    .padding(16);

    if zustand.nachrichten.is_empty() {
        inhalt = inhalt.push(if zustand.gespraech_laedt {
            text("lädt…").size(12)
        } else {
            // Nicht „lädt…": Ein leeres Ergebnis ist ein Befund, kein
            // Zwischenstand. Wer hier ewig „lädt…" zeigt, lässt den Benutzer
            // auf etwas warten, das nie kommt.
            text("keine Nachrichten in diesem Gespräch").size(12)
        });
    }

    for nachricht in &zustand.nachrichten {
        let mut block = column![
            text(nachricht.betreff.clone()).size(14),
            text(format!(
                "{} · {}",
                nachricht.von.adresse,
                zeitpunkt(nachricht.gesendet_am)
            ))
            .size(11),
        ]
        .spacing(2);

        // Textmails bleiben Text, HTML-Mails werden gezeichnet. Welche
        // Form vorliegt, hat der Kern entschieden.
        block = match &nachricht.koerper {
            mail_core::Koerper::Text { inhalt } => block.push(text(inhalt.clone()).size(12)),
            mail_core::Koerper::Html { dokument, .. } => block.push(dokument_zeichnen(dokument)),
        };

        for anhang in &nachricht.anhaenge {
            block = block.push(
                text(format!(
                    "📎 {} · {}",
                    anhang.dateiname,
                    groesse_lesbar(anhang.groesse)
                ))
                .size(11),
            );
        }

        inhalt = inhalt.push(block);
    }

    // Das Antwortfeld steht UNTER dem Verlauf, nicht darüber: Man liest
    // erst, dann schreibt man.
    inhalt = inhalt.push(
        column![
            text("Antwort").size(11),
            text_editor(&zustand.antwort)
                .height(Length::Fixed(120.0))
                .on_action(Nachricht::AntwortBearbeitet),
            if zustand.antwort_laeuft {
                button(text("sendet…").size(12))
            } else {
                button(text("Antwort senden").size(12)).on_press(Nachricht::AntwortSenden)
            },
        ]
        .spacing(4),
    );

    if let Some(hinweis) = &zustand.hinweis {
        inhalt = inhalt.push(text(hinweis.clone()).size(11));
    }

    container(scrollable(container(inhalt).max_width(760.0)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Zeichnet ein Dokument mit den Mitteln von Iced.
///
/// Die Zuordnung ist bewusst schlicht: Absatz, Überschrift, Listenpunkt,
/// Zitat, Trennlinie, Bild, Tabellenzeile. Mehr kennt das Dokumentmodell
/// nicht, und mehr braucht eine Mail nicht. Was ein Newsletter an
/// Spaltenlayout mitbringt, wird zu einer Folge von Blöcken — lesbar, wenn
/// auch nicht pixelgleich. Das ist der bewusste Preis dafür, keinen Browser
/// im Fenster zu haben.
fn dokument_zeichnen(dokument: &Dokument) -> Element<'_, Nachricht> {
    let mut spalte = column![].spacing(6);

    for block in &dokument.bloecke {
        let element: Element<'_, Nachricht> = match block {
            Block::Absatz { abschnitte, zitatebene } => {
                let absatz = rich_text(spans(abschnitte))
                    .size(12)
                    .on_link_click(Nachricht::VerweisGeklickt);

                if *zitatebene > 0 {
                    // Zitate werden eingerückt UND mit einem Strich
                    // versehen: Einrückung allein geht beim Überfliegen
                    // unter, und bei vierfach gestapelten Verläufen bleibt
                    // sonst kein Platz mehr.
                    container(row![text("│").size(12), absatz].spacing(6))
                        .padding(Padding::default().left(10.0 * (*zitatebene as f32)))
                        .into()
                } else {
                    absatz.into()
                }
            }
            Block::Ueberschrift { stufe, abschnitte } => {
                let groesse = match stufe {
                    1 => 18.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                rich_text(spans(abschnitte))
                    .size(groesse)
                    .font(Font { weight: Schriftstaerke::Bold, ..Font::DEFAULT })
                    .on_link_click(Nachricht::VerweisGeklickt)
                    .into()
            }
            Block::Listenpunkt { abschnitte, ebene, nummer } => {
                let zeichen = match nummer {
                    Some(nummer) => format!("{nummer}."),
                    None => "•".to_owned(),
                };
                container(
                    row![
                        text(zeichen).size(12),
                        rich_text(spans(abschnitte))
                            .size(12)
                            .on_link_click(Nachricht::VerweisGeklickt),
                    ]
                    .spacing(6),
                )
                .padding(Padding::default().left(12.0 * (*ebene as f32)))
                .into()
            }
            Block::Trennlinie => container(text("────────").size(11)).into(),
            Block::Bild { quelle, alt } => {
                // Bilder werden NICHT geladen. Ein `cid:`-Bild steckt zwar
                // in der Mail selbst, aber die Anzeige braucht erst den Weg
                // vom Anhang in den Zeichner; alles andere zeigt nach
                // draussen und bliebe auch dann aus.
                let beschriftung = if alt.trim().is_empty() {
                    format!("[Bild: {}]", kurz(quelle))
                } else {
                    format!("[Bild: {alt}]")
                };
                text(beschriftung).size(11).into()
            }
            Block::Tabellenzeile { zellen, kopfzeile } => {
                let mut zeile = row![].spacing(14);
                for zelle in zellen {
                    let inhalt = text(zelle.clone()).size(12);
                    zeile = zeile.push(if *kopfzeile {
                        inhalt.font(Font { weight: Schriftstaerke::Bold, ..Font::DEFAULT })
                    } else {
                        inhalt
                    });
                }
                zeile.into()
            }
        };

        spalte = spalte.push(element);
    }

    spalte.into()
}

fn spans(abschnitte: &[Abschnitt]) -> Vec<iced::widget::text::Span<'_, String>> {
    abschnitte
        .iter()
        .map(|abschnitt| {
            let mut stueck = span(abschnitt.text.clone()).font(Font {
                weight: if abschnitt.fett {
                    Schriftstaerke::Bold
                } else {
                    Schriftstaerke::Normal
                },
                style: if abschnitt.kursiv {
                    Schriftstil::Italic
                } else {
                    Schriftstil::Normal
                },
                ..Font::DEFAULT
            });

            if let Some(ziel) = &abschnitt.verweis {
                stueck = stueck.link(ziel.clone()).underline(true);
            }

            stueck
        })
        .collect()
}

/// Lange Adressen kürzen, damit eine Zeile eine Zeile bleibt.
fn kurz(text: &str) -> String {
    if text.chars().count() <= 48 {
        return text.to_owned();
    }
    let anfang: String = text.chars().take(45).collect();
    format!("{anfang}…")
}

/// Bytes so schreiben, wie ein Mensch sie liest.
fn groesse_lesbar(bytes: u64) -> String {
    match bytes {
        0..=999 => format!("{bytes} B"),
        1_000..=999_999 => format!("{:.1} kB", bytes as f64 / 1_000.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}

/// Eine Haarlinie am rechten Rand, in der Farbe des Themas — nicht fest
/// verdrahtet, sonst sieht sie in einem hellen Thema falsch aus.
fn trennlinie_rechts(thema: &Theme) -> iced::widget::container::Style {
    let farben = thema.extended_palette();
    iced::widget::container::Style {
        border: iced::Border {
            color: farben.background.strong.color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..iced::widget::container::Style::default()
    }
}

/// Unix-Millisekunden als lokale Zeit. Heute nur die Uhrzeit, in diesem Jahr
/// Tag und Monat, davor das ganze Datum — so hält eine dichte Liste mehr
/// Zeilen, ohne Information zu verlieren.
fn zeitpunkt(millisekunden: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};

    let Some(zeit) = Local.timestamp_millis_opt(millisekunden).single() else {
        return String::new();
    };
    let heute = Local::now();

    if zeit.date_naive() == heute.date_naive() {
        zeit.format("%H:%M").to_string()
    } else if zeit.year() == heute.year() {
        zeit.format("%d.%m. %H:%M").to_string()
    } else {
        zeit.format("%d.%m.%Y").to_string()
    }
}

fn spalte_rechts(zustand: &Postfach) -> Element<'_, Nachricht> {
    if zustand.gewaehltes_gespraech.is_some() {
        return spalte_gespraech(zustand);
    }

    let mut inhalt = column![text("Konto hinzufügen").size(13)]
        .spacing(8)
        .padding(16);

    inhalt = inhalt
        .push(feld("Bezeichnung", &zustand.formular.beschriftung, Feld::Beschriftung))
        .push(feld("E-Mail-Adresse", &zustand.formular.adresse, Feld::Adresse))
        .push(feld("IMAP-Server", &zustand.formular.host, Feld::Host))
        .push(feld("Port", &zustand.formular.port, Feld::Port))
        .push(feld("Benutzername (leer = Adresse)", &zustand.formular.benutzer, Feld::Benutzer))
        .push(feld("SMTP-Server (leer = wie IMAP)", &zustand.formular.smtp_host, Feld::SmtpHost))
        .push(feld("SMTP-Port (leer = 465)", &zustand.formular.smtp_port, Feld::SmtpPort))
        .push(
            text_input("Passwort", &zustand.formular.passwort)
                .on_input(|wert| Nachricht::Feld(Feld::Passwort, wert))
                // Nicht nur Kosmetik: ein Passwort, das im Klartext im
                // Fenster steht, landet in jedem Screenshot und auf jedem
                // Bildschirm, über den jemand hinwegsieht.
                .secure(true),
        )
        .push(button(text("Anlegen").size(13)).on_press(Nachricht::Anlegen));

    if let Some(hinweis) = &zustand.hinweis {
        inhalt = inhalt.push(text(hinweis.clone()).size(12));
    }

    inhalt = inhalt.push(
        text("Verbinden kommt mit Aufgabe #9863 — das Konto wird noch nicht geprüft.").size(11),
    );

    container(scrollable(container(inhalt).max_width(460.0)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn feld<'a>(beschriftung: &'a str, wert: &'a str, feld: Feld) -> Element<'a, Nachricht> {
    column![
        text(beschriftung).size(11),
        text_input("", wert).on_input(move |wert| Nachricht::Feld(feld, wert)),
    ]
    .spacing(2)
    .into()
}

/// Wo die lokale Datenbank liegt. Auf dem Desktop darf sich das die App
/// aussuchen; auf Mobile weist die Plattform das Verzeichnis zu, weshalb der
/// Kern es als Parameter nimmt und nie selbst bestimmt.
fn datenverzeichnis() -> String {
    std::env::var("XDG_DATA_HOME")
        .map(|basis| format!("{basis}/rustymail"))
        .unwrap_or_else(|_| {
            let heim = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            format!("{heim}/.local/share/rustymail")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legt ein Konto gegen den lokalen GreenMail-Container an — im ECHTEN
    /// Datenverzeichnis und im ECHTEN Schlüsselbund, damit die App beim
    /// nächsten Start etwas zu zeigen hat.
    ///
    /// Das ist kein Prüfpunkt, sondern ein Werkzeug: Ein Fenster lässt sich
    /// von hier aus nicht anklicken, und ein Konto, das schon da ist, macht
    /// den Weg ohne Klick sichtbar.
    ///
    /// ```sh
    /// cargo test -p rustymail-desktop --features test-support \
    ///     -- --ignored demo_konto --nocapture
    /// ```
    #[test]
    #[ignore = "legt ein echtes Konto an — nur von Hand"]
    fn demo_konto_gegen_greenmail_anlegen() {
        let laufzeit = tokio::runtime::Runtime::new().expect("Laufzeit");
        let kern = Kern::neu(
            Einrichtung { datenverzeichnis: datenverzeichnis() },
            Arc::new(Schluesselbund),
            Arc::new(Protokollhoerer),
        )
        .expect("Kern");

        laufzeit.block_on(async {
            // Vorhandene zuerst weg, sonst sammeln sich bei jedem Lauf neue.
            for konto in kern.konten().await.expect("lesen") {
                kern.konto_entfernen(konto.id).await.expect("entfernen");
            }

            let id = kern
                .konto_anlegen(Kontoentwurf {
                    beschriftung: "GreenMail (lokal)".into(),
                    adresse: "test@localhost".into(),
                    protokoll: Protokoll::Imap,
                    verbindung: Verbindungsdaten {
                        host: "localhost".into(),
                        port: 3993,
                        benutzer: "test".into(),
                        verschluesselung: Verschluesselung::Tls,
                        anmeldeart: mail_core::Anmeldeart::Passwort,
                        jmap_sitzung: None,
                        smtp_host: Some("localhost".into()),
                        smtp_port: Some(3465),
                    },
                    geheimnis: "geheim".into(),
                })
                .await
                .expect("anlegen");

            println!("Konto {id} angelegt");
        });
    }

    /// Geht die Kette der Oberfläche gegen das ECHTE Datenverzeichnis durch:
    /// Konten → Ordner → Abgleich → Gespräche → Gespräch öffnen.
    ///
    /// Ein Fenster lässt sich von hier aus nicht verlässlich anklicken (der
    /// Zeiger rechnet in einem anderen Koordinatenraum als der Compositor).
    /// Dieser Test ersetzt den Klick, nicht die Augen: Er prüft, ob der Kern
    /// mit DIESER Datenbank dieselben Antworten gibt wie im Testverzeichnis.
    #[test]
    #[ignore = "braucht Container und angelegtes Demo-Konto"]
    fn die_kette_der_oberflaeche_gegen_das_echte_verzeichnis() {
        let laufzeit = tokio::runtime::Runtime::new().expect("Laufzeit");
        let kern = Kern::neu(
            Einrichtung { datenverzeichnis: datenverzeichnis() },
            Arc::new(Schluesselbund),
            Arc::new(Protokollhoerer),
        )
        .expect("Kern");

        laufzeit.block_on(async {
            let konten = kern.konten().await.expect("Konten");
            assert_eq!(konten.len(), 1, "erst `demo_konto…` laufen lassen");

            let ordner = kern.ordner(konten[0].id.clone()).await.expect("Ordner");
            let eingang = ordner
                .iter()
                .find(|o| o.rolle == Ordnerrolle::Eingang)
                .expect("Eingang");
            println!("Ordnerkennung: {}", eingang.id);

            kern.abgleichen(eingang.id.clone()).await.expect("Abgleich");

            let seite = kern
                .threads(eingang.id.clone(), None, 100)
                .await
                .expect("Gespräche");
            println!("Gespräche: {}", seite.zeilen.len());
            for zeile in &seite.zeilen {
                println!("  thread_id={:?} betreff={:?}", zeile.thread_id, zeile.betreff);
            }

            let strang = seite.zeilen.first().expect("mindestens ein Gespräch");
            let nachrichten = kern
                .gespraech(eingang.id.clone(), strang.thread_id.clone())
                .await
                .expect("Gespräch");
            println!("Nachrichten im ersten Gespräch: {}", nachrichten.len());

            assert!(
                !nachrichten.is_empty(),
                "Das Gespräch {:?} im Ordner {:?} liefert nichts",
                strang.thread_id,
                eingang.id
            );
        });
    }

    /// Prüft den ECHTEN Tresor des Betriebssystems, nicht eine Nachbildung.
    ///
    /// `#[ignore]`, weil dafür ein Schlüsselbund-Dienst laufen muss — auf
    /// einem Bau-Server ohne Sitzung gibt es keinen, und ein Test, der dort
    /// rot ist, wird irgendwann abgeschaltet statt gelesen. Von Hand:
    ///
    /// ```sh
    /// cargo test -p rustymail-desktop -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "braucht einen laufenden Schlüsselbund-Dienst"]
    fn der_echte_tresor_schreibt_liest_und_loescht() {
        let tresor = Schluesselbund;
        let kennung = "probe:rustymail:test".to_owned();

        // Vorher sauber, auch wenn ein voriger Lauf abgebrochen ist.
        tresor.loesche(kennung.clone()).expect("vorher aufräumen");
        assert_eq!(tresor.lies(kennung.clone()).expect("lesen"), None);

        tresor
            .schreib(kennung.clone(), "geheim".to_owned())
            .expect("schreiben");
        assert_eq!(
            tresor.lies(kennung.clone()).expect("lesen"),
            Some("geheim".to_owned())
        );

        tresor.loesche(kennung.clone()).expect("löschen");
        assert_eq!(
            tresor.lies(kennung).expect("lesen"),
            None,
            "Der Eintrag ist nach dem Löschen noch da"
        );
    }
}
