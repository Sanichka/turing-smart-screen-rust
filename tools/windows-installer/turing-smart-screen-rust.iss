; Turing Smart Screen (Rust) Inno Setup script.
; Build the portable folder first:  powershell -File dist.ps1
; Then compile:  iscc tools\windows-installer\turing-smart-screen-rust.iss
;   (override version:  iscc /DMyAppVersion=0.2.0 ...)
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#define SourceDir "..\..\dist\turing-smart-screen-rust\"
#define IconDir "..\..\res\icons\monitor-icon-17865\"
#define WizardDir "..\..\tools\windows-installer\"

#define MyAppName "Turing Smart Screen (Rust)"
#define MyAppPublisher "turing-smart-screen-rust contributors"
#define MyAppURL "https://github.com/Sanichka/turing-smart-screen-rust"

[Setup]
AppId={{3A7C9E21-6B4D-4C2A-9E0F-7D1A2B3C4D5E6}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputBaseFilename=turing-smart-screen-rust-{#MyAppVersion}-windows
AllowNoIcons=yes
PrivilegesRequired=admin
Compression=lzma
SolidCompression=yes
WizardStyle=modern
SetupIconFile={#IconDir}icon.ico
LicenseFile=..\..\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#SourceDir}turing-smart-screen.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}turing-configure.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}config.yaml"; DestDir: "{app}"; Check: not FileExists(ExpandConstant('{app}\config.yaml'))
Source: "{#SourceDir}res\*"; DestDir: "{app}\res"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}version.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\external\PawnIO\PawnIO_setup.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{group}\Turing Smart Screen (configure)"; Filename: "{app}\turing-configure.exe"; IconFilename: "{#IconDir}icon.ico"
Name: "{group}\Uninstall Turing Smart Screen"; Filename: "{uninstallexe}"

[Run]
; Sensor driver (one-time, needs the elevation this setup already has).
Filename: "{tmp}\PawnIO_setup.exe"; Description: "Install sensor driver (CPU temp/fan)"; Flags: postinstall skipifsilent
; Logon task with highest privileges; the daemon finds its files next to the exe.
Filename: "schtasks.exe"; Parameters: "/Create /TN ""Turing Smart Screen"" /TR ""'{app}\turing-smart-screen.exe' --daemon"" /SC ONLOGON /DELAY 0000:30 /RL HIGHEST /F"; Description: "Start monitor at logon"; Flags: postinstall runhidden
