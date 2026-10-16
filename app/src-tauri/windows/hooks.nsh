; Installer hooks for the per-user NSIS setup.exe (tauri.windows.conf.json).
;
; They do by hand what `morphring integrate --install` does from Rust
; (crates/platform/src/explorer.rs): the "Convert with MorphRing" verb for
; files and folders under HKCU, and the Send To shortcut. Keep the two in step;
; the windows-app job in release.yml greps this file for every key path in
; explorer.rs, so drifting apart fails the release instead of a user's Explorer.
;
; Like the Rust side, a verb key is only written or removed when it is missing
; or carries MorphRingManaged=1. Somebody else's MorphRing key (no idea who,
; but the registry is a shared fridge) is left alone.

!include LogicLib.nsh

; Writes one verb key, the NSIS twin of explorer::verb_entries.
!macro _MORPHRING_WRITE_VERB KEY
  Push $R0
  Push $R1
  ClearErrors
  EnumRegKey $R0 HKCU "${KEY}" 0
  ${If} ${Errors}
    StrCpy $R1 "1" ; the key does not exist yet, so it is ours to make
  ${Else}
    ReadRegStr $R1 HKCU "${KEY}" "MorphRingManaged"
  ${EndIf}
  ${If} $R1 == "1"
    ; Marker first, same reason as in explorer.rs: a half-written key is
    ; still recognisably ours the next time around.
    WriteRegStr HKCU "${KEY}" "MorphRingManaged" "1"
    WriteRegStr HKCU "${KEY}" "" "Convert with MorphRing"
    WriteRegStr HKCU "${KEY}" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe"
    WriteRegStr HKCU "${KEY}" "MultiSelectModel" "Player"
    WriteRegStr HKCU "${KEY}\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --convert "%1"'
  ${Else}
    DetailPrint "Leaving HKCU\${KEY} alone: it was not created by MorphRing"
  ${EndIf}
  Pop $R1
  Pop $R0
!macroend

; Removes one verb key, but only if it carries our marker.
!macro _MORPHRING_REMOVE_VERB KEY
  Push $R0
  ClearErrors
  ReadRegStr $R0 HKCU "${KEY}" "MorphRingManaged"
  ${IfNot} ${Errors}
  ${AndIf} $R0 == "1"
    DeleteRegKey HKCU "${KEY}"
  ${EndIf}
  Pop $R0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro _MORPHRING_WRITE_VERB "Software\Classes\*\shell\MorphRing"
  !insertmacro _MORPHRING_WRITE_VERB "Software\Classes\Directory\shell\MorphRing"

  ; Send To hands every selected file over in one command line, which the
  ; verb (one process per file) cannot. Both roads end at --convert.
  CreateDirectory "$APPDATA\Microsoft\Windows\SendTo"
  CreateShortCut "$APPDATA\Microsoft\Windows\SendTo\MorphRing.lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "--convert" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWNORMAL "" "Convert with MorphRing"

  ; Tell Explorer the context menu changed, so it does not need a sign-out.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro _MORPHRING_REMOVE_VERB "Software\Classes\*\shell\MorphRing"
  !insertmacro _MORPHRING_REMOVE_VERB "Software\Classes\Directory\shell\MorphRing"
  Delete "$APPDATA\Microsoft\Windows\SendTo\MorphRing.lnk"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend
