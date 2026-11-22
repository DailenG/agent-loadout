; Loadout NSIS installer hooks (SPEC 16.1), included by windows/installer.nsi
; through bundle.windows.nsis.installerHooks.
;
; The per-user install folder holds loadout.exe next to the app, so the
; installer adds that folder to the user PATH (HKCU\Environment) unless it
; is already there, and a standalone uninstall takes it out again: the
; folder is deleted, so an entry for it is of no use.
;
; Two cases keep the entry where it is:
; - an update (`/UPDATE`, the in-app updater);
; - an uninstall the installer runs before reinstalling or upgrading (its
;   "uninstall before installing" choice). The installer starts the old
;   uninstaller in place with `_?=<install dir>`, while a standalone
;   uninstall (Settings > Apps, winget) first copies itself to %TEMP%; so an
;   uninstaller whose own folder is the install folder was started by the
;   installer, and the PATH entry, including its position, stays.
;
; A user PATH that does not fit in an NSIS string (NSIS_MAX_STRLEN, 1024
; characters in the NSIS that Tauri uses) is never rewritten: ReadRegStr
; then returns an empty string with the error flag set, which must not be
; mistaken for "no Path value". The value's existence is checked with
; RegGetValueW, and a value that exists but cannot be read leaves PATH
; untouched with a message saying what to do by hand.

!include "WinMessages.nsh"
!include "LogicLib.nsh"
!include "WordFunc.nsh"

!define LOADOUT_PATH_TOO_LONG "Your user PATH is too long for the installer to edit safely, so it was left unchanged."

!macro NSIS_HOOK_POSTINSTALL
  Call LoadoutAddToUserPath
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ${If} $EXEDIR == $INSTDIR
      DetailPrint "Uninstall run by the installer: $INSTDIR stays on your user PATH."
    ${Else}
      Call un.LoadoutRemoveFromUserPath
    ${EndIf}
  ${EndIf}
!macroend

; Reads HKCU\Environment\Path into $0. Leaves $1 = "ok" (value read, or
; absent and $0 empty) or "too-long" (the value exists but did not fit).
!macro LOADOUT_READ_USER_PATH
  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  StrCpy $1 "ok"
  ${If} ${Errors}
    StrCpy $0 ""
    ; RegGetValueW(HKEY_CURRENT_USER, "Environment", "Path",
    ; RRF_RT_ANY | RRF_NOEXPAND, NULL, NULL, &size): 2 is ERROR_FILE_NOT_FOUND.
    System::Call 'advapi32::RegGetValueW(p 0x80000001, w "Environment", w "Path", i 0x1000ffff, p 0, p 0, *i 0) i .r1'
    ${If} $1 = 2
      StrCpy $1 "ok"
    ${Else}
      StrCpy $1 "too-long"
    ${EndIf}
  ${EndIf}
!macroend

Function LoadoutAddToUserPath
  Push $0
  Push $1
  Push $2

  !insertmacro LOADOUT_READ_USER_PATH
  ${If} $1 == "too-long"
    Goto too_long
  ${EndIf}

  StrLen $1 $0
  StrLen $2 $INSTDIR
  IntOp $1 $1 + $2
  IntOp $1 $1 + 2
  ${If} $1 >= ${NSIS_MAX_STRLEN}
    Goto too_long
  ${EndIf}

  ; Already on PATH (an earlier install, or added by hand).
  ${WordReplace} ";$0;" ";$INSTDIR;" ";" "+" $1
  ${If} $1 != ";$0;"
    Goto done
  ${EndIf}

  ${If} $0 == ""
    StrCpy $0 "$INSTDIR"
  ${Else}
    StrCpy $1 $0 1 -1
    ${If} $1 == ";"
      ; Keep the trailing separator, so uninstall restores the value exactly.
      StrCpy $0 "$0$INSTDIR;"
    ${Else}
      StrCpy $0 "$0;$INSTDIR"
    ${EndIf}
  ${EndIf}
  WriteRegExpandStr HKCU "Environment" "Path" $0
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  DetailPrint "Added $INSTDIR to your user PATH."
  Goto done

  too_long:
  DetailPrint "${LOADOUT_PATH_TOO_LONG}"
  MessageBox MB_ICONINFORMATION|MB_OK "${LOADOUT_PATH_TOO_LONG}$\r$\n$\r$\nTo run loadout from a terminal, add this folder to it by hand:$\r$\n$INSTDIR" /SD IDOK

  done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function un.LoadoutRemoveFromUserPath
  Push $0
  Push $1

  !insertmacro LOADOUT_READ_USER_PATH
  ${If} $1 == "too-long"
    Goto too_long
  ${EndIf}
  ${If} $0 == ""
    Goto done
  ${EndIf}
  StrLen $1 $0
  IntOp $1 $1 + 2
  ${If} $1 >= ${NSIS_MAX_STRLEN}
    Goto too_long
  ${EndIf}

  ${WordReplace} ";$0;" ";$INSTDIR;" ";" "+" $1
  ${If} $1 == ";$0;"
    Goto done
  ${EndIf}
  ; Drop the separators added around the value above.
  StrCpy $1 $1 "" 1
  StrCpy $1 $1 -1
  WriteRegExpandStr HKCU "Environment" "Path" $1
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  Goto done

  too_long:
  DetailPrint "${LOADOUT_PATH_TOO_LONG}"
  MessageBox MB_ICONINFORMATION|MB_OK "${LOADOUT_PATH_TOO_LONG}$\r$\n$\r$\nRemove this folder from it by hand:$\r$\n$INSTDIR" /SD IDOK

  done:
  Pop $1
  Pop $0
FunctionEnd
