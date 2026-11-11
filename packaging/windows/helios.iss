; The Windows installer of Helios, an Inno Setup script compiled by
; packaging/windows/release.ps1 (Inno Setup 6):
;
;   ISCC /DVersion=X.Y.Z /DSource=TREE /DOutDir=OUT packaging/windows/helios.iss
;
; TREE is the folder the portable zip holds (the programs, COPYING,
; THIRD-PARTY-LICENSES.txt, README.md). The installer is per-user and
; needs no administrator: it installs into %LOCALAPPDATA%\Programs\Helios,
; adds a Start menu entry (a desktop one on request) and an uninstaller,
; listed in Settings, Apps. Silent use: /VERYSILENT /SUPPRESSMSGBOXES
; [/DIR=FOLDER]; the uninstaller (unins000.exe in that folder) takes the
; same flags. Uninstalling leaves the user's libraries and settings alone.

#ifndef Version
  #error Version is not defined: pass /DVersion=X.Y.Z
#endif
#ifndef Source
  #error Source is not defined: pass /DSource=FOLDER
#endif
#ifndef OutDir
  #define OutDir "."
#endif

[Setup]
; The installation's identity: never change it, or a new version installs
; beside the old one instead of over it.
AppId={{833F5226-CF08-47A0-BF08-0B2A9389FFC3}
AppName=Helios
AppVersion={#Version}
AppVerName=Helios {#Version}
AppPublisher=Helios contributors
AppPublisherURL=https://github.com/quazardous/helios
AppSupportURL=https://github.com/quazardous/helios/issues
AppUpdatesURL=https://github.com/quazardous/helios/releases
; Per-user, no elevation: {autopf} is %LOCALAPPDATA%\Programs.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Helios
DefaultGroupName=Helios
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutDir}
OutputBaseFilename=helios-{#Version}-windows-x86_64-setup
SetupIconFile=helios.ico
UninstallDisplayIcon={app}\helios-gui.exe
UninstallDisplayName=Helios {#Version}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; A running Helios is closed before its files are replaced.
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Helios"; Filename: "{app}\helios-gui.exe"; Comment: "Photo editor"
Name: "{autodesktop}\Helios"; Filename: "{app}\helios-gui.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\helios-gui.exe"; Description: "{cm:LaunchProgram,Helios}"; Flags: nowait postinstall skipifsilent
