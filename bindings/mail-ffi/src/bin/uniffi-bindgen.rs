//! Der Binding-Erzeuger als Teil des Projekts statt als globale Installation.
//!
//! Eine global installierte `uniffi-bindgen` muss in der Fassung zur
//! `uniffi`-Abhängigkeit passen. Weicht sie ab, entstehen Bindings, die
//! kompilieren und zur Laufzeit falsch liegen — der unangenehmste Fehler, den
//! diese Grenze hergibt. Hier kann sie nicht abweichen.

fn main() {
    uniffi::uniffi_bindgen_main()
}
