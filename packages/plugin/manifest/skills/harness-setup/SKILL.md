---
name: harness-setup
description: Richtet den Befehl nullradix im normalen Terminal ein, damit er überall ohne Claude Code funktioniert, und führt bei Bedarf das Einrichten des Projekts und das Öffnen des Boards durch. Verwenden, wenn nullradix im Terminal nicht gefunden wird, nach der ersten Installation des Plugins oder nach einem Plugin-Update.
---

# nullradix im Terminal einrichten

Der Benutzer soll dafür nichts von Hand tun. Du führst die Schritte selbst aus und fragst nur, wo unten ausdrücklich steht, dass du fragst.

## 1. Installieren

Führe aus:

```
nullradix install
```

Der Befehl kopiert das Binary nach `~/.nullradix/bin`, legt die Startdateien dort an und ergänzt den PATH des Benutzers (nur Benutzerebene, nie systemweit). Er richtet außerdem das Plugin in Claude Code ein. Er ist wiederholbar und aktualisiert die Kopie nach einem Plugin-Update.

- Meldet er, dass er nur aus dem gebauten Plugin läuft, kommt der Aufruf nicht aus dem installierten Plugin. Sage dem Benutzer, dass er im Repository einmal `nullradix install` ausführen soll, und beende.
- Meldet er einen Fehler beim PATH, führe `nullradix install --no-path` aus und nenne dem Benutzer den Ordner `~/.nullradix/bin`, den er selbst in den PATH aufnehmen muss.

## 2. Ergebnis prüfen

Prüfe die Kopie, ohne den PATH zu brauchen:

```
nullradix --help
```

Nenne dem Benutzer in einem Satz: Neue Terminals kennen `nullradix` jetzt in jedem Ordner. Bereits offene Terminals müssen neu geöffnet werden.

## 3. Projekt einrichten (nur auf Wunsch)

Ist das aktuelle Verzeichnis ein Git-Projekt ohne NULLRADIX, frage einmal: „Soll ich dieses Projekt jetzt einrichten und das Board öffnen?“ Bei „Ja“:

```
nullradix init
nullradix board
```

`nullradix init` legt außerdem die lokalen Rules für Doku-Regeln und Board-Regel an (Board-Regel: Tickets, Board, Fragen und Notizen meinen immer das NULLRADIX-Board); in schon eingerichteten Projekten ergänzt ein erneutes `nullradix init` fehlende Rules. `nullradix init` nennt, welches Board dem Repo zugeordnet ist. Ist keines zugeordnet, zeige dem Benutzer die Boards mit `nullradix board boards list` und frage, welches zu diesem Repo gehört; ordne es dann mit `nullradix board boards repo <id> <pfad>` zu (Pfad = Hauptordner des Repos). Fehlt die Board-Konfiguration, verweise auf Einstellungen → Board-Datenbank in der Oberfläche.

Bei „Nein“ erwähne die Befehle nur und höre auf. Richte nie ungefragt ein Projekt ein; NULLRADIX startet nur dort, wo der Benutzer es will.

## Regeln

- Tickets nie von Hand ändern; dieser Skill legt keine Tickets an.
- Ändere keine Dateien außerhalb von `~/.nullradix` und dem PATH des Benutzers.
- Schreibe Texte für Menschen in natürlichem Deutsch ohne wörtlich übersetzte Fachwörter.
