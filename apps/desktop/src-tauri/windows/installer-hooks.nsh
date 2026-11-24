; Hooks des NSIS-Installers: tragen den Installationsordner (dort liegt nullradix.exe) in den
; Benutzer-PATH ein und beim Deinstallieren wieder aus, damit `nullradix` im Terminal gefunden wird.
; Gesetzt über PowerShell wie in crates/nullradix-core/src/install/windows_user_path.rs: kein
; direkter Registry-Zugriff aus NSIS, dessen Zeichenketten lange PATH-Werte abschneiden würden,
; und `%VAR%`-Einträge bleiben unverändert. Der Ordner kommt über die Umgebungsvariable
; NULLRADIX_BIN_DIR, damit kein Quoting nötig ist. `$$` ist ein `$` für PowerShell.

!define NULLRADIX_PATH_READ "$$d = $$env:NULLRADIX_BIN_DIR.TrimEnd([char]92); \
$$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $$true); \
$$p = [string]$$k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); \
$$items = @($$p -split ';' | Where-Object { $$_ -ne '' }); \
$$rest = @($$items | Where-Object { $$_.TrimEnd([char]92) -ne $$d })"

; Löscht eine nicht vorhandene Variable; löst dabei die Änderungsmeldung an neue Terminals aus
!define NULLRADIX_PATH_NOTIFY "[Environment]::SetEnvironmentVariable('NULLRADIX_PATH_REFRESH', $$null, 'User')"

!define NULLRADIX_PATH_ADD "${NULLRADIX_PATH_READ}; \
if ($$rest.Count -ne $$items.Count) { exit 0 }; \
$$k.SetValue('Path', (($$items + $$d) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString); \
${NULLRADIX_PATH_NOTIFY}"

!define NULLRADIX_PATH_REMOVE "${NULLRADIX_PATH_READ}; \
if ($$rest.Count -eq $$items.Count) { exit 0 }; \
$$k.SetValue('Path', ($$rest -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString); \
${NULLRADIX_PATH_NOTIFY}"

; Backticks begrenzen die Zeile, weil das Skript ' und der Aufruf " enthält; ein Plugin-Aufruf
; nimmt sonst stillschweigend nur das Stück bis zum ersten ' als Befehl
!macro NULLRADIX_RUN_PATH_SCRIPT SCRIPT
  System::Call 'Kernel32::SetEnvironmentVariable(t "NULLRADIX_BIN_DIR", t "$INSTDIR")i'
  ; Fehlschlag bricht die Installation nicht ab; $0 bleibt für das Hauptskript erhalten
  Push $0
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "${SCRIPT}"`
  Pop $0
  Pop $0
!macroend

; Programme aus dem Installationsordner. Tauri beendet vor dem Kopieren nur, was nullradix-desktop.exe
; benutzt – und zwar alles, auch die Dispatch-Worker (sie laufen als `nullradix-desktop.exe
; dispatch-worker`). Ein abgelöster `nullradix serve` oder ein `nullradix` aus dem PATH überlebt dagegen und
; sperrt nullradix.exe; dann scheitern Deinstallation und Installation.
; Bei einem Update beendet nullradix-processes.ps1 deshalb nur die App und `serve` und benennt Dateien,
; die Worker oder kurze Aufrufe noch sperren, um. Die Worker laufen aus der umbenannten Datei weiter,
; Tauris Prüfung findet unter dem alten Namen niemanden mehr, und die neue Datei landet daneben.
; Nur wer wirklich deinstalliert, beendet alles – nach Rückfrage, wenn Dispatch-Läufe aktiv sind.
; Das Skript ist für NSIS-Zeichenketten (höchstens 1024 Zeichen) zu lang und kommt als Datei mit.
; __FILEDIR__ muss hier oben ausgewertet werden; in einem Makro wäre es der Ordner von installer.nsi.
!define NULLRADIX_HOOKS_DIR "${__FILEDIR__}"

; Führt nullradix-processes.ps1 im angegebenen Modus aus; der Exit-Code steht danach in $0
!macro NULLRADIX_RUN_PROCESS_SCRIPT MODE
  InitPluginsDir
  File "/oname=$PLUGINSDIR\nullradix-processes.ps1" "${NULLRADIX_HOOKS_DIR}\nullradix-processes.ps1"
  System::Call 'Kernel32::SetEnvironmentVariable(t "NULLRADIX_BIN_DIR", t "$INSTDIR")i'
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\nullradix-processes.ps1" -Mode ${MODE}`
  Pop $0
!macroend

; Update ohne Deinstallation. Ist NULLRADIX schon installiert, zeigt Tauri nach der Willkommensseite eine
; Seite „Vor dem Installieren deinstallieren / Nicht deinstallieren“; einstellen lässt sie sich nicht.
; Sie überspringt sich aber selbst, wenn unter dem Uninstall-Schlüssel kein `UninstallString` steht.
; Beim Verlassen der Willkommensseite (diese Datei wird vor den Seiten eingebunden) legt der
; Installer den Wert deshalb unter NullradixUninstallString beiseite. Die Installation schreibt ihn neu;
; bricht sie ab oder scheitert, kommt der alte Wert zurück. Im Passiv-Modus (Updater der App) gibt es
; keine Willkommensseite, dort überspringt Tauri die Seite ohnehin.
; Die Funktionen stehen vor den Var- und define-Zeilen der Vorlage: Sie dürfen weder $PassiveMode &
; Co. noch ${UNINSTKEY} benutzen, darum steht der Schlüssel hier ausgeschrieben (Benutzerinstallation,
; productName „NULLRADIX“). Belegt Tauri einen der beiden MUI-Haken einmal selbst, bricht der Build ab.
!define /ifndef NULLRADIX_UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\NULLRADIX"
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE NullradixWelcomeLeave
!define MUI_CUSTOMFUNCTION_ABORT NullradixRestoreUninstallString

Function NullradixWelcomeLeave
  Push $0
  Push $1
  ; Läuft die App, sagt ein kurzer Hinweis, dass sie beendet wird und die Dispatch-Läufe weiterlaufen
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT workers
  StrCpy $1 $0
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT servers
  ; „error“ statt einer Zahl (PowerShell fehlt) zählt wie 0
  IntCmp $0 0 nullradix_hidden nullradix_hidden
  StrCpy $0 "NULLRADIX läuft gerade und wird für das Update beendet."
  IntCmp $1 1 nullradix_one_run nullradix_hint nullradix_many_runs
  nullradix_one_run:
  StrCpy $0 "$0$\r$\n$\r$\nDer laufende Dispatch-Lauf läuft weiter; sein Ergebnis wertet NULLRADIX nach dem Neustart aus."
  Goto nullradix_hint
  nullradix_many_runs:
  StrCpy $0 "$0$\r$\n$\r$\nDie $1 laufenden Dispatch-Läufe laufen weiter; ihre Ergebnisse wertet NULLRADIX nach dem Neustart aus."
  nullradix_hint:
  MessageBox MB_OKCANCEL|MB_ICONINFORMATION $0 /SD IDOK IDOK nullradix_hidden
  Pop $1
  Pop $0
  ; Abbrechen: auf der Willkommensseite bleiben
  Abort
  nullradix_hidden:
  ReadRegStr $0 SHCTX "${NULLRADIX_UNINSTKEY}" "UninstallString"
  StrCmp $0 "" nullradix_done
  WriteRegStr SHCTX "${NULLRADIX_UNINSTKEY}" "NullradixUninstallString" $0
  DeleteRegValue SHCTX "${NULLRADIX_UNINSTKEY}" "UninstallString"
  nullradix_done:
  Pop $1
  Pop $0
FunctionEnd

Function NullradixRestoreUninstallString
  Push $0
  ReadRegStr $0 SHCTX "${NULLRADIX_UNINSTKEY}" "NullradixUninstallString"
  StrCmp $0 "" +3
  WriteRegStr SHCTX "${NULLRADIX_UNINSTKEY}" "UninstallString" $0
  DeleteRegValue SHCTX "${NULLRADIX_UNINSTKEY}" "NullradixUninstallString"
  Pop $0
FunctionEnd

Function .onInstFailed
  Call NullradixRestoreUninstallString
FunctionEnd

; Beendet App und `serve`, räumt gesperrte Dateien beiseite und leert die Ressourcen-Ordner
!macro NSIS_HOOK_PREINSTALL
  Push $0
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT install
  Pop $0
!macroend

; Der Deinstaller gehört zu einem Update, wenn er mit /UPDATE läuft oder wenn ihn der Installer
; („Vor dem Installieren deinstallieren“) mit `_?=` direkt aus dem Ordner startet; sonst kopiert
; er sich nach %TEMP%. Bei einem echten Deinstallieren fragt er nach, wenn Dispatch-Läufe aktiv
; sind; still gilt „Ja“. Die Sprungmarken stehen nur einmal im Uninstall-Abschnitt.
!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  StrCmp $UpdateMode 1 nullradix_update
  StrCmp $EXEDIR $INSTDIR nullradix_update
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT workers
  ; „error“ statt einer Zahl (PowerShell fehlt) zählt wie 0
  IntCmp $0 0 nullradix_uninstall nullradix_uninstall
  MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 \
    "Es laufen noch $0 Dispatch-Läufe von NULLRADIX. Sie werden abgebrochen, wenn du NULLRADIX jetzt deinstallierst.$\r$\n$\r$\nTrotzdem deinstallieren?" \
    /SD IDYES IDYES nullradix_uninstall
  Pop $0
  Abort "Abgebrochen: Es laufen noch Dispatch-Läufe."
  nullradix_uninstall:
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT uninstall
  Goto nullradix_done
  nullradix_update:
  !insertmacro NULLRADIX_RUN_PROCESS_SCRIPT update
  nullradix_done:
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Die Installation hat UninstallString neu geschrieben; der beiseite gelegte Wert ist überflüssig
  DeleteRegValue SHCTX "${NULLRADIX_UNINSTKEY}" "NullradixUninstallString"
  !insertmacro NULLRADIX_RUN_PATH_SCRIPT "${NULLRADIX_PATH_ADD}"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro NULLRADIX_RUN_PATH_SCRIPT "${NULLRADIX_PATH_REMOVE}"
!macroend
