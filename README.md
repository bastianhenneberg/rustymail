# RustyMail

Ein eigener Mail-Client für Desktop und Mobile. GitHub: `bastianhenneberg/rustymail`. Rust-Kern, native
Oberflächen.

Diese App steht **für sich**. Sie hat nichts mit `peppermint/mailbox` zu tun:
Das Paket bleibt, wo es ist, und bedient die Online-Produkte weiter. Hier gibt
es keine Mitte, keine Zuweisung und keine Werkzeugleiste aus einem fremden
System.

## Aufbau

```
crates/mail-core     Fassade: Befehle rein, Ansichten raus, Ereignisse hoch
crates/mail-proto    IMAP/SMTP und JMAP hinter EINEM Vertrag
                     (Kennungen sind Zeichenketten — JMAP hat keine UIDs)
crates/mail-store    SQLite: Schema, Wanderungen, FTS5-Suche
crates/mail-sync     Abgleich: UIDVALIDITY, CONDSTORE, Warteschlange
crates/mail-crypto   S/MIME und OpenPGP
bindings/mail-ffi    UniFFI-Hülle für iOS und Android (staticlib + cdylib)
apps/desktop         Iced-Oberfläche (Binary: `rustymail`): Linux, macOS, Windows aus einem Binary
```

Der Desktop linkt `mail-core` direkt — eine FFI-Grenze gibt es nur zu Swift
und Kotlin.

| Plattform | Oberfläche |
|---|---|
| Linux, macOS, Windows | Iced (selbst gezeichnet, keine WebView) |
| iOS | SwiftUI |
| Android | Compose |

## Bauen

```sh
cargo test                      # Kern und Speicher, braucht nichts weiter
cargo run -p rustymail-desktop  # Desktop-App
```

Der Kern baut **ohne NDK, ohne Xcode und ohne JVM**, und das soll so bleiben:
Es ist der Unterschied zwischen „ich kann die Logik prüfen" und „ich erfahre
nach 15 Minuten Cloud-Build, ob sie stimmt".

### Gegen einen echten IMAP-Server testen

Die Protokollschicht wird gegen **GreenMail** im Container geprüft, nicht
gegen ein Produktiv-Postfach. Dort liegt Kundenpost, und ein Test, der
Flaggen setzt und verschiebt, hat darin nichts zu suchen:

```sh
docker run -d --name rustymail-greenmail -p 3025:3025 -p 3143:3143 -p 3993:3993 \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.users=test:geheim@localhost -Dgreenmail.hostname=0.0.0.0' \
  greenmail/standalone:latest

# Testpost einliefern (SMTP, Klartext auf 3025) — siehe docs/testpost.py
cargo test -p mail-proto --features test-support -- --ignored
cargo test -p mail-core --test konto_gegen_greenmail -- --ignored
```

Der LOGIN-Name ist **`test`**, nicht `test@localhost` — GreenMail legt den
Nutzer unter der Adresse an, nimmt aber nur den Teil davor als Anmeldenamen.

GreenMail stellt sich sein Zertifikat selbst aus. Damit das nicht zu einem
dauerhaften Loch führt, liegt der ungeprüfte TLS-Weg hinter dem Feature
`test-support` und in einem eigenen Modul (`crates/mail-proto/src/tls.rs`) —
in einem Release-Build existiert dieser Code nicht. Eine Flagge an einer
Funktion hätte gereicht, um ihn versehentlich zu setzen; ein Feature muss
jemand in eine `Cargo.toml` schreiben.

### JMAP gegen Stalwart testen

GreenMail spricht kein JMAP. Für die zweite Erfüllung des Vertrags läuft ein
zweiter Container:

```sh
docker run -d --name rustymail-stalwart -p 18080:8080 \
  -v rustymail-stalwart-daten:/opt/stalwart stalwartlabs/stalwart:latest

# Das Startkennwort steht EINMALIG im Protokoll:
docker logs rustymail-stalwart | grep -A3 password

RUSTYMAIL_STALWART_PASSWORT=… \
  cargo test -p mail-proto --test jmap_gegen_stalwart -- --ignored
```

Testpost legt man dort über JMAP selbst ab (`Blob/upload` + `Email/import`) —
siehe `docs/jmap_testpost.py`. Über SMTP ginge es auch, verlangt aber eine
eingerichtete Domain.

### Bindings erzeugen

```sh
cargo build -p mail-ffi --release
cargo run -p mail-ffi --bin uniffi-bindgen -- generate \
    --library target/release/libmail_ffi.so \
    --language kotlin --out-dir bindings/kotlin/generated
```

`uniffi-bindgen` liegt absichtlich **im Projekt** und wird nicht global
installiert: Eine Fassung, die nicht zur `uniffi`-Abhängigkeit passt, erzeugt
Bindings, die kompilieren und zur Laufzeit falsch liegen.

## Regeln, die nicht verhandelbar sind

**An der FFI-Grenze.** Keine Panik nach draußen — eine Panik über `extern "C"`
ist undefiniertes Verhalten, über JNI bringt sie die Laufzeitumgebung um; auf
dem Gerät sieht beides aus wie ein Absturz ohne Ursache. Deshalb gibt der Kern
Fehler zurück und hat kein `todo!()`.

**Grobkörnig schneiden.** Befehle rein, fertige Ansichten raus, Änderungen als
Ereignis. Keine Feldzugriffe: Ein `betreff(id)` wird pro Zeile einmal gerufen,
und bei 200 Zeilen sind das 200 Grenzübertritte für einen Bildschirm.

**Zugangsdaten nie im Kern.** Passwörter und Token liegen im Tresor der
Plattform — iOS Keychain, Android Keystore, auf Linux der Secret Service. Der
Kern bekommt sie für die Dauer einer Verbindung über das `Tresor`-Merkmal.

**TLS über rustls + rustls-platform-verifier.** Nicht `native-tls`: das zieht
OpenSSL in jedes Kreuzkompilat für iOS und Android.

**OpenPGP mit der RustCrypto-Rückseite.** Nicht Nettle — das zieht GMP als
C-Bibliothek nach, und die muss dann für vier Mobilziele kreuzkompiliert
werden.

**Sync-Zustand von Anfang an.** `uidvalidity` je Ordner, `highestmodseq` für
die Flaggen, und eine Warteschlange für alles, was lokal passiert ist. Wer das
nachträglich einbaut, schreibt die Abgleich-Maschine neu.

**Kein Browser im Fenster.** Der Mailtext wird selbst gezeichnet. Eine WebView
brächte eine Skript-Laufzeit und einen eigenen Netzzugang in die Anzeige
fremder Post.

## AI Brain

Projekt-Slug: `rustymail`
