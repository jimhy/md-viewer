; MD Viewer - Inno Setup installer script
; Version is injected by release.py via /DMyAppVersion=x.y.z
; Build manually: ISCC.exe /DMyAppVersion=1.0.2 installer.iss

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif

#define MyAppName      "MD Viewer"
#define MyAppPublisher "MD Viewer"
#define MyAppURL       "https://github.com/"
#define MyAppExeName   "md-viewer.exe"

[Setup]
AppId={{A8F9B3C2-7D2E-4F1A-9C3B-1D7E4F8A2B6C}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
VersionInfoVersion={#MyAppVersion}

DefaultDirName={autopf}\MD Viewer
DefaultGroupName=MD Viewer
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName} {#MyAppVersion}

OutputDir=dist
OutputBaseFilename=md-viewer-setup-v{#MyAppVersion}
SetupIconFile=icon.ico

Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
MinVersion=10.0
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
AssocMd=Associate .md and .markdown files with {#MyAppName}

[Tasks]
Name: "associate"; Description: "{cm:AssocMd}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "md-viewer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "LICENSE";       DestDir: "{app}"; Flags: ignoreversion
Source: "THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.md";     DestDir: "{app}"; Flags: ignoreversion
Source: "README_CN.md";  DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}";                Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}";          Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; ProgID definition
Root: HKA; Subkey: "Software\Classes\MDViewer.Document"; ValueType: string; ValueName: ""; ValueData: "Markdown Document"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MDViewer.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MDViewer.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associate

; Extension associations
Root: HKA; Subkey: "Software\Classes\.md";       ValueType: string; ValueName: ""; ValueData: "MDViewer.Document"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.markdown"; ValueType: string; ValueName: ""; ValueData: "MDViewer.Document"; Flags: uninsdeletevalue; Tasks: associate

; "Open with MD Viewer" context menu entry (works for any file extension)
Root: HKA; Subkey: "Software\Classes\*\shell\Open with MD Viewer"; ValueType: string; ValueName: ""; ValueData: "Open with MD Viewer"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\*\shell\Open with MD Viewer"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\*\shell\Open with MD Viewer\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associate

; Applications entry (for "Open with..." dialog)
Root: HKA; Subkey: "Software\Classes\Applications\{#MyAppExeName}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#MyAppExeName}\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#MyAppExeName}\SupportedTypes"; ValueType: string; ValueName: ".markdown"; ValueData: ""; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
