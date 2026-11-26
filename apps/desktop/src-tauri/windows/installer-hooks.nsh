; DevSpace NSIS installer hooks (Tauri `bundle.windows.nsis.installerHooks`).
;
; The installer is per user (no administrator rights): it adds the
; installation directory, which holds `devspace.exe`, to the PATH of the
; current user, and the uninstaller removes it. PowerShell edits the
; user-level PATH only and broadcasts the change to running programs.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$dir = \"$INSTDIR\"; $$path = [Environment]::GetEnvironmentVariable(\"Path\", \"User\"); $$parts = @(); if ($$path) { $$parts = $$path.Split(\";\") | Where-Object { $$_ -ne \"\" } }; if ($$parts -notcontains $$dir) { [Environment]::SetEnvironmentVariable(\"Path\", (($$parts + $$dir) -join \";\"), \"User\") }"'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$dir = \"$INSTDIR\"; $$path = [Environment]::GetEnvironmentVariable(\"Path\", \"User\"); if ($$path) { $$parts = $$path.Split(\";\") | Where-Object { $$_ -ne \"\" -and $$_ -ne $$dir }; [Environment]::SetEnvironmentVariable(\"Path\", ($$parts -join \";\"), \"User\") }"'
!macroend
