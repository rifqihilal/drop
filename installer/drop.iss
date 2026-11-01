; Builds target\installer\Drop-<version>-Setup.exe from the release build:
;   cargo build --release
;   ISCC.exe installer\drop.iss

#define Version GetVersionNumbersString("..\target\release\drop.exe")

[Setup]
AppId=DropNotch
AppName=Drop
AppVersion={#Version}
AppPublisher=Hilal
DefaultDirName={autopf}\Drop
DefaultGroupName=Drop
DisableProgramGroupPage=yes
DisableDirPage=auto
OutputDir=..\target\installer
OutputBaseFilename=Drop-{#Version}-Setup
SetupIconFile=..\assets\drop.ico
UninstallDisplayIcon={app}\drop.exe
UninstallDisplayName=Drop
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UsedUserAreasWarning=no
CloseApplications=no

[Languages]
Name: "id"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
AutoStart=Mulai Drop bersama Windows
StartNow=Jalankan Drop
RemoveData=Hapus juga pengaturan, daftar tugas, dan Screen Time Drop?%n%nPilih No untuk menyimpannya.

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"

[Files]
Source: "..\target\release\drop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; DestName: "NOTICE.txt"; Flags: ignoreversion
Source: "..\assets\fonts\OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Drop"; Filename: "{app}\drop.exe"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "DropNotch"; ValueData: """{app}\drop.exe"""; Tasks: autostart

[Run]
Filename: "{app}\drop.exe"; Description: "{cm:StartNow}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[Code]
procedure CloseDrop;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im drop.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseDrop;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  CloseDrop;
  Result := True;
end;

procedure CurUninstallStepChanged(Step: TUninstallStep);
begin
  if Step = usPostUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'DropNotch');
    if SuppressibleMsgBox(CustomMessage('RemoveData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      DelTree(ExpandConstant('{userappdata}\Drop'), True, True, True);
  end;
end;
