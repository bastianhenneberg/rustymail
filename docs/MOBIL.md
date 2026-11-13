# iOS und Android anbinden

Der Kern ist fertig und die FFI-Grenze erzeugt saubere Bindings. Was fehlt,
sind die beiden Oberflächen — und die Werkzeugketten, um sie zu bauen.

Dieses Dokument ist die Anleitung dafür, nicht eine Absichtserklärung: Jeder
Punkt darin ist entweder gemessen oder stammt aus einem Fehler, der schon
einmal Zeit gekostet hat.

## Was heute schon steht

```sh
cargo build -p mail-ffi
cargo run -p mail-ffi --bin uniffi-bindgen -- generate \
    --library target/debug/libmail_ffi.so --language kotlin --out-dir bindings/kotlin/generated
```

Erzeugt rund 5.000 Zeilen Kotlin. Was Android daraus bekommt:

| | |
|---|---|
| `KernInterface` | 17 Methoden, alle `suspend` — Konten, Ordner, Gespräche, Antworten, Flaggen, Archivieren, Warteschlange |
| `EreignisHoerer` | Rückruf-Schnittstelle: Änderungen kommen gepusht, nicht gepollt |
| `Tresor` | von der Plattform zu implementieren — auf Android der Keystore |
| `sealed class Block` | das Dokumentmodell des Mailtexts, fertig zum `when` in Compose |
| `sealed class Fehler` | als `kotlin.Exception`, mit den Fällen, die die Oberfläche unterscheiden muss |

Für Swift dasselbe mit `--language swift` gegen `libmail_ffi.a`.

## Was auf diesem Rechner fehlt

Stand 12.11.2026 gemessen:

| | |
|---|---|
| Android-NDK | fehlt (`ANDROID_HOME` zeigt auf ein nicht vorhandenes Verzeichnis) |
| `cargo-ndk` | fehlt |
| `kotlinc`, `gradle`, `adb` | fehlen |
| `rustup` | verwaltet dieses Rust **nicht** — es ist das Paket von Arch |

Der letzte Punkt ist der unangenehmste: `rustup target add aarch64-linux-android`
geht damit nicht. Entweder kommt rustup dazu (und dann muss klar sein, welches
`cargo` im PATH gewinnt), oder die Mobil-Builds laufen woanders.

## Android, wenn die Kette steht

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release -p mail-ffi
```

Vier Dinge, die sonst reissen — alle aus dem Wiki-Artikel
„Nativen Code in ein Expo-Modul einbinden":

1. **Gradle findet `cargo` nicht über den PATH der JVM.** Ein `environment 'PATH', …`
   an der Exec-Task genügt nicht; es gehört der absolute Pfad hin.
2. **`crate-type` nicht breiter deklarieren als nötig.** `mail-ffi` steht auf
   `["staticlib", "cdylib"]` — cargo baut jede deklarierte Form, und
   `cargo-ndk` kopiert jede `.so` nach `jniLibs`, auch eine nie geladene.
3. **`rustls_platform_verifier::android::init_with_env()`** muss beim App-Start
   einmal aufgerufen werden, bevor irgendetwas ins Netz geht — sonst findet der
   Zertifikatsprüfer die JVM nicht, und zwar erst auf dem Gerät.
   Dazu eine Proguard-Regel: `-keep class org.rustls.platformverifier.** { *; }`
4. **`catch_unwind` an der FFI-Grenze.** Eine Panik über JNI hinaus bringt die
   Laufzeitumgebung um; auf dem Gerät sieht das aus wie ein Absturz ohne Ursache.
   Der Kern gibt deshalb Fehler zurück und hat kein `todo!()`.

## iOS, wenn ein Mac da ist

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p mail-ffi --target aarch64-apple-ios
cargo run -p mail-ffi --bin uniffi-bindgen -- generate \
    --library target/aarch64-apple-ios/release/libmail_ffi.a --language swift --out-dir bindings/swift/generated
```

- **Der Simulator ist ein eigenes Ziel.** `aarch64-apple-ios` ist das Gerät,
  `aarch64-apple-ios-sim` der Simulator auf Apple Silicon.
- **Ein statischer CocoaPods-Pod linkt nicht**, er sammelt nur Objektdateien.
  `OTHER_LDFLAGS` gehört in `user_target_xcconfig`, nicht in
  `pod_target_xcconfig` — sonst kommen „Undefined symbols" aus einem Build, in
  dem sonst alles geklappt hat.

## Was die Oberflächen selbst mitbringen müssen

Der Kern lässt bewusst offen, was plattformeigen ist:

| | iOS | Android |
|---|---|---|
| `Tresor` | Keychain | Keystore |
| Datenverzeichnis | von der Plattform zugewiesen — der Kern sucht sich keines aus | dito |
| Datumsformat | `DateFormatter`, lokalisiert | `DateTimeFormatter` |
| Mailtext | SwiftUI über `Block` | Compose über `Block` |
| Benachrichtigungen | APNs — **braucht einen serverseitigen Watcher**, siehe #9866 | FCM |

Der letzte Punkt ist kein Detail: iOS erlaubt keine dauerhafte
IMAP-Verbindung im Hintergrund. Ohne einen Dienst, der IDLE hält und per APNs
weckt, gibt es keine Benachrichtigungen — nur Nachsehen beim Öffnen.
