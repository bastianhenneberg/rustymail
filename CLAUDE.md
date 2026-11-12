# RustyMail

Eigener Mail-Client: Rust-Kern, native Oberflächen für Desktop, iOS und
Android. Aufbau, Bauanleitung und die nicht verhandelbaren Regeln stehen in
[README.md](README.md) — vor der ersten Änderung lesen.

## AI Brain

Projekt-Slug: `rustymail`

## Was diese App NICHT ist

Sie ist **nicht** `peppermint/mailbox` als App. Das Paket bedient die
Online-Produkte und bleibt dort. Hier gibt es keine Mitte, keine Zuweisung,
keine gemeinsame Werkzeugleiste und keinen Postfach-Assistenten. Wer eine
Fähigkeit aus dem Paket hierher holen will, braucht dafür eine Entscheidung,
keine Analogie.

## Sprache im Code

Fachbegriffe deutsch, wie in den übrigen Peppermint-Paketen: `Ordner`,
`Kopfzeilen`, `Flagge`, `Schlummer`, `Ergebnis`, `Fehler`. Technische
Allgemeinbegriffe bleiben, wo sie herkommen (`Task`, `Element`, `uid`).

## Beim Arbeiten

- `cargo test` läuft ohne NDK, Xcode oder JVM. Das ist Absicht: Jede Änderung,
  die den Kern von einer Plattform-Toolchain abhängig macht, nimmt diese
  Eigenschaft weg.
- Die Desktop-App braucht ein Display. In einer Session ohne
  `WAYLAND_DISPLAY` startet sie nicht — das ist die Session, nicht die App.
- Neue Wanderungen in `crates/mail-store/src/schema/` als **neue** Datei.
  Bestehende nie ändern: Auf Geräten, die sie schon angewandt haben, läuft
  die Änderung nicht nach, und dort entsteht ein Schema, das es nirgends
  sonst gibt.
