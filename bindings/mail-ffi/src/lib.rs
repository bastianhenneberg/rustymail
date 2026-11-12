//! Die FFI-Hülle. Enthält keine Logik — nur das, was die Grenze braucht.
//!
//! Der Kern bleibt dadurch plattformneutral und auf einem gewöhnlichen
//! Rechner baubar und testbar: ohne NDK, ohne Xcode, ohne JVM.
//!
//! Bindings erzeugen (ohne irgendwas global zu installieren):
//!
//! ```sh
//! cargo build -p mail-ffi --release
//! cargo run -p mail-ffi --bin uniffi-bindgen -- generate \
//!     --library target/release/libmail_ffi.a \
//!     --language swift --out-dir bindings/swift/generated
//! cargo run -p mail-ffi --bin uniffi-bindgen -- generate \
//!     --library target/release/libmail_ffi.so \
//!     --language kotlin --out-dir bindings/kotlin/generated
//! ```
//!
//! Und die zwei Dinge, die beim Einbinden regelmäßig Zeit kosten:
//!
//! - **Der Simulator ist ein eigenes Ziel.** `aarch64-apple-ios` ist das
//!   Gerät, `aarch64-apple-ios-sim` der Simulator auf Apple Silicon.
//! - **Ein statischer CocoaPods-Pod linkt nicht**, er sammelt nur
//!   Objektdateien. `OTHER_LDFLAGS` gehört deshalb in
//!   `user_target_xcconfig`, nicht in `pod_target_xcconfig` — sonst kommen
//!   „Undefined symbols" aus einem Build, in dem sonst alles geklappt hat.

pub use mail_core::*;
