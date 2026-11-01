#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef PayloadDir
  #error PayloadDir is required
#endif
#ifndef OutputDirectory
  #error OutputDirectory is required
#endif

[Setup]
AppId={{5978F67A-B4B4-4C80-A4EA-0E9D371E31AF}
AppName=随手传
AppVersion={#AppVersion}
AppPublisher=LAN Relay
AppPublisherURL=https://github.com/kroblus/lan-relay
DefaultDirName={localappdata}\Programs\LanRelay
DefaultGroupName=随手传
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.18362
OutputDir={#OutputDirectory}
OutputBaseFilename=LanRelay-{#AppVersion}-windows-x64-setup
SetupIconFile={#PayloadDir}\LanRelay.ico
UninstallDisplayIcon={app}\LanRelay.exe
AppMutex=Local\LanRelay.Desktop
CloseApplications=no
RestartApplications=no
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UsePreviousAppDir=yes

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "快捷方式："; Flags: checkedonce

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userprograms}\随手传"; Filename: "{app}\LanRelay.exe"
Name: "{userdesktop}\随手传"; Filename: "{app}\LanRelay.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\LanRelay.exe"; Description: "打开随手传"; Flags: nowait postinstall skipifsilent

; User data lives outside {app}. Never add it to [UninstallDelete].
