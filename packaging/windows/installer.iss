#ifndef AppVersion
#define AppVersion "0.3.0"
#endif
#ifndef SourceDirectory
#define SourceDirectory "..\..\output\dist\windows\LanRelay-0.3.0-windows-x64"
#endif

[Setup]
AppId={{792C39F1-3CB6-42CC-8AE6-9A5B46C4E13F}
AppName=随手传
AppVersion={#AppVersion}
AppPublisher=LAN Relay
DefaultDirName={localappdata}\Programs\LanRelayRust
DefaultGroupName=随手传
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=LanRelay-{#AppVersion}-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\lan-relay.exe
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "快捷方式："

[Files]
Source: "{#SourceDirectory}\lan-relay.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDirectory}\relay-server.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDirectory}\relay-migrate.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDirectory}\QUICK_START.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\随手传"; Filename: "{app}\lan-relay.exe"
Name: "{autodesktop}\随手传"; Filename: "{app}\lan-relay.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\lan-relay.exe"; Description: "打开随手传"; Flags: nowait postinstall skipifsilent

; Runtime data is intentionally outside the installation folder. Uninstallation
; never removes %LOCALAPPDATA%\LanRelay\v2 or any V1 data directory.
