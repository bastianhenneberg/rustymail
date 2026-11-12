#!/usr/bin/env python3
"""Liefert Testpost in den GreenMail-Container ein.

Die drei Nachrichten sind nicht beliebig: Die erste trägt Umlaute im Betreff
(und wandert damit als `=?utf-8?q?…?=` über die Leitung), die zweite hängt
per In-Reply-To und References an der ersten, die dritte hat einen Anhang.
Zusammen decken sie das ab, was in der Praxis als Erstes schiefgeht.
"""

import email.utils
import smtplib
from email.message import EmailMessage


def sende(betreff, von, text, message_id, in_reply_to=None, referenzen=None, anhang=False):
    nachricht = EmailMessage()
    nachricht["Subject"] = betreff  # wird automatisch RFC-2047-kodiert
    nachricht["From"] = von
    nachricht["To"] = "test@localhost"
    nachricht["Date"] = email.utils.formatdate()
    nachricht["Message-ID"] = message_id
    if in_reply_to:
        nachricht["In-Reply-To"] = in_reply_to
    if referenzen:
        nachricht["References"] = " ".join(referenzen)
    nachricht.set_content(text)
    if anhang:
        nachricht.add_attachment(
            b"Spalte;Wert\n1;2\n", maintype="text", subtype="csv", filename="zahlen.csv"
        )
    with smtplib.SMTP("127.0.0.1", 3025) as smtp:
        smtp.send_message(nachricht)


if __name__ == "__main__":
    sende(
        "Angebot für Grünflächenpflege — Rückfrage",
        "kunde@beispiel.de",
        "Guten Tag,\n\nkönnen Sie das Angebot anpassen?\n\nViele Grüße",
        "<erste@beispiel.de>",
    )
    sende(
        "Re: Angebot für Grünflächenpflege — Rückfrage",
        "kollege@beispiel.de",
        "Mache ich bis morgen.",
        "<zweite@beispiel.de>",
        in_reply_to="<erste@beispiel.de>",
        referenzen=["<erste@beispiel.de>"],
    )
    # Eine Mail VON der Testadresse an sich selbst: Nur so landet eine
    # Antwort darauf wieder im eigenen Posteingang und lässt sich prüfen.
    sende(
        "Notiz an mich",
        "test@localhost",
        "Nicht vergessen.",
        "<vierte@beispiel.de>",
    )
    sende(
        "Rechnung 2026-1147",
        "buchhaltung@beispiel.de",
        "Anbei die Rechnung.",
        "<dritte@beispiel.de>",
        anhang=True,
    )
    print("Testpost eingeliefert")
