; Per-user installer for 墨笺 (md-work).
;
; Build with:
;   cargo run -p xtask -- package
;   cargo run -p xtask -- installer
; or manually:
;   makensis /DVERSION=1.2.7 /DDIST=<workspace>\dist\md-work-1.2.7-win64 md-work.nsi
;
; Only HKCU is written and nothing is installed machine-wide, so no elevation
; is required. The app's own "用墨笺打开" verb (HKCU\Software\Classes) is left
; untouched here and can be (re)registered from Settings -> Windows.

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "StrFunc.nsh"
${StrRep}

Var WORKSPACE_DIR
Var WORKSPACE_DIR_FIELD

!ifndef VERSION
  !define VERSION "1.2.7"
!endif
!ifndef DIST
  !error "Pass /DDIST=<path to dist\md-work-<ver>-win64>"
!endif

!define APP_NAME "墨笺"
!define APP_SLUG "md-work"
!define APP_PUBLISHER "lidawei"
!define APP_EXE "md-work.exe"
!define APP_CLI "md-work-cli.exe"
!define APP_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\md-work"

Name "${APP_NAME} ${VERSION}"
OutFile "..\..\dist\md-work-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP_SLUG}"
InstallDirRegKey HKCU "Software\${APP_SLUG}" "InstallDir"
RequestExecutionLevel user
Unicode true
SetCompressor /SOLID lzma
BrandingText "${APP_NAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Copyright © 2026 ${APP_PUBLISHER}. MIT License."

!define MUI_ABORTWARNING
; setup.exe 用纸箱图标；卸载程序使用NSIS的默认图标。
!define MUI_ICON "..\..\assets\installer-icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "启动${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
Page custom WorkspacePageCreate WorkspacePageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

LangString WS_TITLE ${LANG_SIMPCHINESE} "选择工作区"
LangString WS_TITLE ${LANG_ENGLISH} "Choose a workspace"
LangString WS_SUBTITLE ${LANG_SIMPCHINESE} "工作区是保存笔记本的文件夹，之后可以在应用里随时切换。"
LangString WS_SUBTITLE ${LANG_ENGLISH} "A workspace is the folder that holds your notebooks. You can switch it later in the app."
LangString WS_EXPLAIN ${LANG_SIMPCHINESE} "工作区就是磁盘上的一个普通文件夹：墨笺在它里面创建多个笔记本，每个笔记本用 Markdown 文件保存笔记。可以选择任意位置（如 D:\笔记），下次启动会自动打开上次使用的工作区。现有设置不会被覆盖。"
LangString WS_EXPLAIN ${LANG_ENGLISH} "A workspace is an ordinary folder. MoJiān creates notebooks inside it and keeps every note as a Markdown file. Pick any location (for example D:\notes); the last workspace opens automatically next time. Existing settings are never overwritten."
LangString WS_FOLDER ${LANG_SIMPCHINESE} "工作区文件夹："
LangString WS_FOLDER ${LANG_ENGLISH} "Workspace folder:"
LangString WS_BROWSE ${LANG_SIMPCHINESE} "浏览…"
LangString WS_BROWSE ${LANG_ENGLISH} "Browse…"
LangString WS_PICK ${LANG_SIMPCHINESE} "选择工作区文件夹"
LangString WS_PICK ${LANG_ENGLISH} "Select the workspace folder"

Function .onInit
  SetShellVarContext current
  StrCpy $WORKSPACE_DIR "$DOCUMENTS\md-work"
FunctionEnd

Function WorkspacePageCreate
  !insertmacro MUI_HEADER_TEXT "$(WS_TITLE)" "$(WS_SUBTITLE)"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 44u "$(WS_EXPLAIN)"
  Pop $0
  ${NSD_CreateLabel} 0 52u 100% 10u "$(WS_FOLDER)"
  Pop $0
  ${NSD_CreateText} 0 64u 78% 14u "$WORKSPACE_DIR"
  Pop $WORKSPACE_DIR_FIELD
  ${NSD_CreateBrowseButton} 80% 64u 20% 14u "$(WS_BROWSE)"
  Pop $0
  ${NSD_OnClick} $0 WorkspacePageBrowse

  nsDialogs::Show
FunctionEnd

Function WorkspacePageBrowse
  Pop $0 ; control HWND pushed by NSD_OnClick
  ${NSD_GetText} $WORKSPACE_DIR_FIELD $0
  nsDialogs::SelectFolderDialog "$(WS_PICK)" "$0"
  Pop $1
  ${If} $1 != error
    ${NSD_SetText} $WORKSPACE_DIR_FIELD "$1"
  ${EndIf}
FunctionEnd

Function WorkspacePageLeave
  ${NSD_GetText} $WORKSPACE_DIR_FIELD $WORKSPACE_DIR
  ${StrRep} $WORKSPACE_DIR "$WORKSPACE_DIR" "$\"" ""
  ${If} $WORKSPACE_DIR == ""
    StrCpy $WORKSPACE_DIR "$DOCUMENTS\md-work"
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "主程序" SEC_MAIN
  SetOutPath "$INSTDIR"
  ; 官方发布文档升级前先去掉旧副本的只读属性，否则 File 覆盖可能失败。
  ${If} ${FileExists} "$INSTDIR\changelog.md"
    SetFileAttributes "$INSTDIR\changelog.md" NORMAL
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\mcp.md"
    SetFileAttributes "$INSTDIR\mcp.md" NORMAL
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\欢迎使用墨笺.md"
    SetFileAttributes "$INSTDIR\欢迎使用墨笺.md" NORMAL
  ${EndIf}
  File "${DIST}\${APP_EXE}"
  File "${DIST}\${APP_CLI}"
  File "${DIST}\changelog.md"
  File "${DIST}\mcp.md"
  File "${DIST}\欢迎使用墨笺.md"
  ; 安装目录里的三份官方文档设为只读；应用内还会用 SHA-256 校验，
  ; 并始终以只读预览模式显示程序内置的权威版本。
  SetFileAttributes "$INSTDIR\changelog.md" READONLY
  SetFileAttributes "$INSTDIR\mcp.md" READONLY
  SetFileAttributes "$INSTDIR\欢迎使用墨笺.md" READONLY
  SetOutPath "$INSTDIR\licenses"
  File "${DIST}\LICENSE"
  File "${DIST}\THIRD-PARTY-NOTICES.md"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\${APP_SLUG}" "InstallDir" "$INSTDIR"

  ; First run only: hand the chosen workspace to the app as a Unicode-safe
  ; registry hint (the app reads it while its config has no workspace root).
  ; Upgrades keep whatever the app already stored in config/runtime.toml or the
  ; legacy config.json.
  ${IfNot} ${FileExists} "$LOCALAPPDATA\md-work\config\runtime.toml"
  ${AndIfNot} ${FileExists} "$LOCALAPPDATA\md-work\config.json"
    WriteRegStr HKCU "Software\${APP_SLUG}" "WorkspaceRoot" "$WORKSPACE_DIR"
  ${EndIf}

  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"

  WriteRegStr HKCU "${APP_KEY}" "DisplayName" "${APP_NAME} ${APP_SLUG} ${VERSION}"
  WriteRegStr HKCU "${APP_KEY}" "DisplayVersion" "${VERSION}"

  WriteRegStr HKCU "${APP_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${APP_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${APP_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${APP_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${APP_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKCU "${APP_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${APP_KEY}" "NoRepair" 1

  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${APP_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_CLI}"
  ${If} ${FileExists} "$INSTDIR\changelog.md"
    SetFileAttributes "$INSTDIR\changelog.md" NORMAL
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\mcp.md"
    SetFileAttributes "$INSTDIR\mcp.md" NORMAL
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\欢迎使用墨笺.md"
    SetFileAttributes "$INSTDIR\欢迎使用墨笺.md" NORMAL
  ${EndIf}
  Delete "$INSTDIR\changelog.md"
  Delete "$INSTDIR\mcp.md"
  Delete "$INSTDIR\欢迎使用墨笺.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\licenses"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${APP_KEY}"
  DeleteRegKey HKCU "Software\${APP_SLUG}"
  ; Notes, settings and the per-user file association stay in place on purpose:
  ; reinstalling picks them up again.
SectionEnd
