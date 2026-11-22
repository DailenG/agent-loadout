; Loadout NSIS installer hooks (SPEC 16.1), included by windows/installer.nsi
; through bundle.windows.nsis.installerHooks.
;
; The per-user install folder holds loadout.exe next to the app, so the
; installer adds that folder to the user PATH (HKCU\Environment) unless it
; is already there, and a real uninstall (not the one an update runs) takes
; it out again: the folder is deleted, so an entry for it is of no use. A
; user PATH too long for NSIS strings is never rewritten (it would be
; truncated); the installer prints what to do by hand instead.

!include "WinMessages.nsh"
!include "LogicLib.nsh"
!include "WordFunc.nsh"

!macro NSIS_HOOK_POSTINSTALL
  Call LoadoutAddToUserPath
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Call un.LoadoutRemoveFromUserPath
  ${EndIf}
!macroend

Function LoadoutAddToUserPath
  Push $0
  Push $1
  Push $2

  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  ${If} ${Errors}
    StrCpy $0 ""
  ${EndIf}

  StrLen $1 $0
  StrLen $2 $INSTDIR
  IntOp $1 $1 + $2
  IntOp $1 $1 + 2
  ${If} $1 >= ${NSIS_MAX_STRLEN}
    DetailPrint "Your user PATH is too long to edit safely; add $INSTDIR to it by hand to run loadout from a terminal."
    Goto done
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

  done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function un.LoadoutRemoveFromUserPath
  Push $0
  Push $1

  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  ${If} ${Errors}
    Goto done
  ${EndIf}
  StrLen $1 $0
  IntOp $1 $1 + 2
  ${If} $1 >= ${NSIS_MAX_STRLEN}
    DetailPrint "Your user PATH is too long to edit safely; remove $INSTDIR from it by hand."
    Goto done
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

  done:
  Pop $1
  Pop $0
FunctionEnd
