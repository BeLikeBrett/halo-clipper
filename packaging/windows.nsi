Unicode true
!include "MUI2.nsh"
!ifndef VERSION
  !define VERSION "0.8.0"
!endif
!ifndef PAYLOAD
  !define PAYLOAD "..\dist\windows"
!endif
!ifndef OUTPUT
  !define OUTPUT "..\dist\Halo-Clipper-Setup.exe"
!endif
Name "Halo Clipper"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\Halo Clipper"
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ABORTWARNING
!define MUI_ICON "..\assets\icon.ico"
!define MUI_UNICON "..\assets\icon.ico"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\halo-clipper.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Halo Clipper"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Section "Halo Clipper"
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /r "${PAYLOAD}\*"
  IfErrors 0 +3
    SetErrorLevel 1
    Abort "Unable to update files. Close Halo Clipper and try again."
  CreateDirectory "$SMPROGRAMS\Halo Clipper"
  CreateShortcut "$SMPROGRAMS\Halo Clipper\Halo Clipper.lnk" "$INSTDIR\halo-clipper.exe" "" "$INSTDIR\halo-clipper.exe" 0
  CreateShortcut "$DESKTOP\Halo Clipper.lnk" "$INSTDIR\halo-clipper.exe" "" "$INSTDIR\halo-clipper.exe" 0
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "DisplayName" "Halo Clipper"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "DisplayIcon" "$INSTDIR\halo-clipper.exe,0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "Publisher" "Halo Clipper"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper" "NoRepair" 1
SectionEnd
Section "Uninstall"
  Delete "$DESKTOP\Halo Clipper.lnk"
  Delete "$SMPROGRAMS\Halo Clipper\Halo Clipper.lnk"
  RMDir "$SMPROGRAMS\Halo Clipper"
  Delete "$INSTDIR\halo-clipper.exe"
  Delete "$INSTDIR\libmpv-2.dll"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  Delete "$INSTDIR\mpv.exe"
  Delete "$INSTDIR\d3dcompiler_43.dll"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  Delete "$INSTDIR\runtime-lock.json"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\HaloClipper"
SectionEnd
