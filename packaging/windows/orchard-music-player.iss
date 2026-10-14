; Windows installer (Inno Setup 6). Built in CI from the staged app folder
; (packaging/stage.sh win-x64 ... dist\orchard-music-player):
;   iscc /DAppVersion=1.0.0 packaging\windows\orchard-music-player.iss
; Output: dist\Orchard-Music-Player-Setup.exe
; Installs for the current user or for all users (the user chooses; /CURRENTUSER
; or /ALLUSERS on the command line), adds a Start menu entry, an optional
; desktop shortcut and an uninstaller. Silent installs:
; /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
; Keep the AppId: later versions upgrade an installation in place with it.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{19F2E30E-5AF0-4452-9447-BF200EDC2117}
AppName=Orchard Music Player
AppVersion={#AppVersion}
AppVerName=Orchard Music Player {#AppVersion}
AppPublisher=Stephan Lengl
AppPublisherURL=https://github.com/sandboxly/orchard-music-player
AppSupportURL=https://github.com/sandboxly/orchard-music-player/issues
AppUpdatesURL=https://github.com/sandboxly/orchard-music-player/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Orchard Music Player
DefaultGroupName=Orchard Music Player
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE
SetupIconFile=..\icons\app.ico
UninstallDisplayIcon={app}\orchard-music-player.ico
UninstallDisplayName=Orchard Music Player
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\..\dist
OutputBaseFilename=Orchard-Music-Player-Setup
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\dist\orchard-music-player\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; The same icon as the one in the executable, as a file: shortcuts and Apps &
; features point at it.
Source: "..\icons\app.ico"; DestDir: "{app}"; DestName: "orchard-music-player.ico"; Flags: ignoreversion

[InstallDelete]
; A new version replaces the license folder completely.
Type: filesandordirs; Name: "{app}\licenses"

[Icons]
Name: "{autoprograms}\Orchard Music Player"; Filename: "{app}\orchard-music-player.exe"; IconFilename: "{app}\orchard-music-player.ico"
Name: "{autodesktop}\Orchard Music Player"; Filename: "{app}\orchard-music-player.exe"; IconFilename: "{app}\orchard-music-player.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\orchard-music-player.exe"; Description: "{cm:LaunchProgram,Orchard Music Player}"; Flags: nowait postinstall skipifsilent
