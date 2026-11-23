; Northroom's Windows setup (Inno Setup 6). scripts/package-windows.sh compiles it and defines
; AppVersion, AppIdentifier, Source (the app folder), Icon, OutputDir and OutputName, plus Sign
; when the setup and uninstaller are signed (`/Snr=<signtool command>`).
;
; Installs per user into %LOCALAPPDATA%\Programs\Northroom with no administrator rights. Each
; identifier has one AppId, so the same setup updates in place. The updater runs it silently
; (gpui/shell/src/updates.rs):
;   /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=<install folder> /WAITPID=<app> [/RELAUNCH=1]
; It waits for the app to exit, installs, and opens the new app when /RELAUNCH=1.

#if AppIdentifier == "com.northroom.app"
  #define AppId "{{692C3AC0-7C0B-4408-A77B-3A9D1A1AEEFE}"
  #define AppTitle "Northroom"
#else
  #define AppId "{{B6C82DF6-ABB0-4235-BB85-7BE58922FA3E}"
  #define AppTitle "Northroom Dev"
#endif

[Setup]
AppId={#AppId}
AppName={#AppTitle}
AppVersion={#AppVersion}
AppVerName={#AppTitle} {#AppVersion}
AppPublisher=Northroom
AppPublisherURL=https://northroom.com
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\{#AppTitle}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; Windows 10 1809 and later: WebView2 and ConPTY.
MinVersion=10.0.17763
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\Northroom.exe
UninstallDisplayName={#AppTitle}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
#ifdef Sign
SignTool=nr
SignedUninstaller=yes
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[InstallDelete]
; Replace Resources whole so files dropped from a release don't linger.
Type: filesandordirs; Name: "{app}\Resources"

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppTitle}"; Filename: "{app}\Northroom.exe"
Name: "{autodesktop}\{#AppTitle}"; Filename: "{app}\Northroom.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Northroom.exe"; Flags: nowait; Check: Relaunching
Filename: "{app}\Northroom.exe"; Description: "{cm:LaunchProgram,{#AppTitle}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  SYNCHRONIZE = $00100000;
  WAIT_SECONDS = 60;

function OpenProcess(Access: Cardinal; Inherit: Integer; Id: Cardinal): Cardinal;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: Cardinal; Milliseconds: Cardinal): Cardinal;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: Cardinal): Integer;
  external 'CloseHandle@kernel32.dll stdcall';

{ Waits for the app that started the update to exit, so its files can be replaced. }
function InitializeSetup(): Boolean;
var
  Id: Cardinal;
  Handle: Cardinal;
begin
  Id := StrToIntDef(ExpandConstant('{param:WAITPID|0}'), 0);
  if Id <> 0 then
  begin
    Handle := OpenProcess(SYNCHRONIZE, 0, Id);
    if Handle <> 0 then
    begin
      WaitForSingleObject(Handle, WAIT_SECONDS * 1000);
      CloseHandle(Handle);
    end;
  end;
  Result := True;
end;

function Relaunching(): Boolean;
begin
  Result := ExpandConstant('{param:RELAUNCH|0}') = '1';
end;
