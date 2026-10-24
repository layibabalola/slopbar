; SlopBar Ultra release installers only (scripts/publish-ultra-release.ps1 adds this file).
; Agents run slopbar-mcp.exe for their whole session, and a running image cannot be written, so a
; quiet update would skip it and leave the old build. Windows does let it be renamed: set it aside
; so the installer writes the new one. Running servers carry on from the old copy; the app deletes
; old copies once nothing runs them (src-tauri/src/mcp_update.rs).
!macro NSIS_HOOK_PREINSTALL
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  Push $R6
  Push $R7
  IfFileExists "$INSTDIR\slopbar-mcp.exe" 0 slopbar_mcp_done
    ${GetTime} "" "L" $R0 $R1 $R2 $R3 $R4 $R5 $R6
    StrCpy $R7 0
    slopbar_mcp_next:
      IntOp $R7 $R7 + 1
      IfFileExists "$INSTDIR\slopbar-mcp.old-$R2$R1$R0-$R4$R5$R6-$R7.exe" slopbar_mcp_next
    ClearErrors
    Rename "$INSTDIR\slopbar-mcp.exe" "$INSTDIR\slopbar-mcp.old-$R2$R1$R0-$R4$R5$R6-$R7.exe"
    ClearErrors
  slopbar_mcp_done:
  Pop $R7
  Pop $R6
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
!macroend

; The uninstaller removes slopbar-mcp.exe itself; copies set aside are not in its list. One an
; agent still runs is left, and so is the folder then. Updates never uninstall.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Delete "$INSTDIR\slopbar-mcp.old-*.exe"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
