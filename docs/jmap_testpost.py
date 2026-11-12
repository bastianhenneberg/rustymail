#!/usr/bin/env python3
"""Legt Testpost in den Stalwart-Container — über JMAP selbst.

Über SMTP ginge es auch, verlangt aber eine eingerichtete Domain. `Blob/upload`
plus `Email/import` braucht nichts ausser dem Verwalterkonto, das Stalwart
beim ersten Start anlegt.

    RUSTYMAIL_STALWART_PASSWORT=… python3 docs/jmap_testpost.py
"""

import base64
import json
import os
import urllib.request

BASIS = "http://localhost:18080"
PASSWORT = os.environ["RUSTYMAIL_STALWART_PASSWORT"]
ANMELDUNG = base64.b64encode(f"admin:{PASSWORT}".encode()).decode()


def ruf(pfad, daten=None, typ="application/json"):
    anfrage = urllib.request.Request(
        BASIS + pfad,
        data=daten,
        headers={"Authorization": f"Basic {ANMELDUNG}", "Content-Type": typ},
    )
    with urllib.request.urlopen(anfrage) as antwort:
        return json.loads(antwort.read())


def jmap(konto, methode, argumente):
    return ruf(
        "/jmap",
        json.dumps(
            {
                "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
                "methodCalls": [[methode, {"accountId": konto, **argumente}, "0"]],
            }
        ).encode(),
    )["methodResponses"][0][1]


if __name__ == "__main__":
    sitzung = ruf("/jmap/session")
    konto = sitzung["primaryAccounts"]["urn:ietf:params:jmap:mail"]

    # Der Betreff kommt bewusst RFC-2047-kodiert — so wie über die Leitung.
    nachricht = (
        "From: kunde@beispiel.de\r\n"
        "To: admin@localhost\r\n"
        "Subject: =?utf-8?q?Angebot_f=C3=BCr_Gr=C3=BCnfl=C3=A4chenpflege?=\r\n"
        "Message-ID: <jmap-erste@beispiel.de>\r\n"
        "Date: Wed, 12 Nov 2026 10:00:00 +0000\r\n"
        "\r\nKoennen Sie das Angebot anpassen?\r\n"
    ).encode()

    blob = ruf(f"/jmap/upload/{konto}/", nachricht, "message/rfc822")
    postfaecher = jmap(konto, "Mailbox/get", {"ids": None, "properties": ["id", "role"]})
    eingang = next(p["id"] for p in postfaecher["list"] if p.get("role") == "inbox")

    ergebnis = jmap(
        konto,
        "Email/import",
        {"emails": {"e1": {"blobId": blob["blobId"], "mailboxIds": {eingang: True}, "keywords": {}}}},
    )
    print("eingelegt:", json.dumps(ergebnis.get("created", {}))[:120])
