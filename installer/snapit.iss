; Snap It: setup .exe (Inno Setup 6).
;
; Built by scripts/package.ps1, which passes the version and the folder holding
; snapit.exe:  ISCC /DAppVersion=0.1.0 /DDistDir=..\dist installer\snapit.iss
;
; Per-user install by default (no admin prompt, %LOCALAPPDATA%\Programs\Snap It),
; with an option on the first page to install for all users instead.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef DistDir
  #define DistDir "..\dist"
#endif

[Setup]
; Never change AppId: it is how upgrades find the existing install.
AppId={{8299F4C6-8354-45DB-8D95-A33CB1E3B108}
AppName=Snap It
AppVersion={#AppVersion}
AppVerName=Snap It {#AppVersion}
AppPublisher=Snap It
AppPublisherURL=https://github.com/abyssalsoldier/snap-it
AppSupportURL=https://github.com/abyssalsoldier/snap-it/issues
AppUpdatesURL=https://github.com/abyssalsoldier/snap-it/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Snap It
VersionInfoDescription=Snap It setup
DefaultDirName={autopf}\Snap It
DefaultGroupName=Snap It
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#DistDir}
OutputBaseFilename=SnapIt-{#AppVersion}-setup-x64
SetupIconFile=..\assets\snapit.ico
UninstallDisplayIcon={app}\snapit.exe
UninstallDisplayName=Snap It
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Snap It is a tray app: [Code] below asks a running copy to quit (snapit --quit) before
; files are replaced or removed; Restart Manager is the fallback for anything still open.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#DistDir}\snapit.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Snap It"; Filename: "{app}\snapit.exe"; Comment: "Screenshots, annotation and screen recording"
Name: "{autodesktop}\Snap It"; Filename: "{app}\snapit.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\snapit.exe"; Description: "{cm:LaunchProgram,Snap It}"; Flags: nowait postinstall skipifsilent

[Registry]
; The app's own opt-in Explorer verbs; never created here, only removed on uninstall.
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\image\shell\SnapIt.Edit"; Flags: dontcreatekey uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\image\shell\SnapIt.Pin"; Flags: dontcreatekey uninsdeletekey

[UninstallDelete]
; The app's own opt-in "Start with Windows" shortcut.
Type: files; Name: "{userstartup}\Snap It.lnk"

[Code]
const
  SnapItMutex = 'Local\SnapIt.SingleInstance';

{ Asks a running Snap It to exit (it forwards --quit to the running copy) and waits up to 5 s. }
procedure CloseSnapIt();
var
  Exe: String;
  Code, I: Integer;
begin
  if not CheckForMutexes(SnapItMutex) then
    Exit;
  Exe := ExpandConstant('{app}\snapit.exe');
  if FileExists(Exe) then
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  for I := 1 to 50 do
  begin
    if not CheckForMutexes(SnapItMutex) then
      Exit;
    Sleep(100);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseSnapIt();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  CloseSnapIt();
  Result := True;
end;
