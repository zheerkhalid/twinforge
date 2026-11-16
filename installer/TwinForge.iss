; TwinForge installer (Inno Setup 6). Per-user: no administrator rights needed.
; Setup and the uninstaller never touch the files TwinForge was used on.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{8C2E5D71-3B94-4F06-9E1A-5A7C0D2B6F43}
AppName=TwinForge
AppVersion={#AppVersion}
AppVerName=TwinForge {#AppVersion}
AppPublisher=Zheer Khalid
AppPublisherURL=https://zheer.dev
AppSupportURL=https://github.com/zheerkhalid/twinforge
AppUpdatesURL=https://github.com/zheerkhalid/twinforge/releases
DefaultDirName={autopf}\TwinForge
DefaultGroupName=TwinForge
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=TwinForge-Setup
SetupIconFile=..\target\twinforge.ico
UninstallDisplayIcon={app}\TwinForge.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Zheer Khalid (zheer.dev)
VersionInfoDescription=TwinForge Setup

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "foldermenu"; Description: "Add ""Find duplicates with TwinForge"" to the right-click menu of folders"; GroupDescription: "Explorer:"; Flags: unchecked

[Files]
Source: "..\target\release\TwinForge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; DestName: "NOTICE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\TwinForge"; Filename: "{app}\TwinForge.exe"
Name: "{group}\Uninstall TwinForge"; Filename: "{uninstallexe}"
Name: "{autodesktop}\TwinForge"; Filename: "{app}\TwinForge.exe"; Tasks: desktopicon

[Registry]
; Per-user folder menu entry; removed again on uninstall.
Root: HKA; Subkey: "Software\Classes\Directory\shell\TwinForge"; ValueType: string; ValueName: ""; ValueData: "Find duplicates with TwinForge"; Flags: uninsdeletekey; Tasks: foldermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\TwinForge"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\TwinForge.exe,0"; Tasks: foldermenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\TwinForge\command"; ValueType: string; ValueName: ""; ValueData: """{app}\TwinForge.exe"" ""%1"""; Tasks: foldermenu

[Run]
Filename: "{app}\TwinForge.exe"; Description: "{cm:LaunchProgram,TwinForge}"; Flags: nowait postinstall skipifsilent

; There is deliberately no [UninstallDelete] section: settings, the
; fingerprint cache and the action history in %LOCALAPPDATA%\TwinForge stay
; where they are, and so does every file TwinForge moved for you.
