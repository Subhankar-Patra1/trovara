; Trovara's additions to the installer (Tauri's NSIS hooks, TRD §15.3). Updates run the installer with /UPDATE and
; skip all of this: the index, the model and fast catch-up stay as they are.

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ; Fast catch-up's scheduled task (TRD §9.3) was set up with administrator permission: taking it away asks for it
    ; again, once, and only when it's there.
    ${If} ${FileExists} "$INSTDIR\trovara-helper.exe"
      ExecWait '"$INSTDIR\trovara-helper.exe" usn uninstall --if-installed'
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; "Delete the application data" (F-UPD-04): Trovara's own data folder too, its index, model, settings and logs.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$LOCALAPPDATA\Trovara"
  ${EndIf}
!macroend
