; Windows installer for fracst (Inno Setup 6). The release workflow builds it:
;   iscc /DAppVersion=0.2.1 /DSourceDir=<folder with the exes> /DOutputBase=<name> fracst.iss
; It installs for the current user without asking for administrator rights
; (or for everyone, if chosen), adds Start menu entries, and optionally puts
; fracst-cli on PATH and opens .kfr files with fracst.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputBase
  #define OutputBase "fracst-setup"
#endif

[Setup]
AppId={{6C1E2F43-8B0A-4E52-9D7C-3F1A5B0E7D21}
AppName=Fracst
AppVersion={#AppVersion}
AppVerName=Fracst {#AppVersion}
AppPublisher=Alex Crișan
AppPublisherURL=https://github.com/alexcrisan1611/fracst
AppSupportURL=https://github.com/alexcrisan1611/fracst/issues
AppUpdatesURL=https://github.com/alexcrisan1611/fracst/releases
DefaultDirName={autopf}\fracst
DefaultGroupName=Fracst
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
ChangesAssociations=yes
LicenseFile=..\..\LICENSE-MIT
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\fracst.exe
OutputDir=.
OutputBaseFilename={#OutputBase}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: desktopicon; Description: "Create a &desktop shortcut"; Flags: unchecked
Name: addtopath; Description: "Add fracst-cli and fracst-server to &PATH"
Name: kfr; Description: "Open .&kfr location files with Fracst"

[Files]
Source: "{#SourceDir}\fracst.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\fracst-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\fracst-server.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; Earlier versions named the shortcuts "fracst"; overwriting them would keep
; that lowercase file name, so remove them and create them afresh.
Type: files; Name: "{autoprograms}\fracst.lnk"
Type: files; Name: "{autodesktop}\fracst.lnk"

[Icons]
Name: "{autoprograms}\Fracst"; Filename: "{app}\fracst.exe"; Comment: "Interactive deep-zoom Mandelbrot explorer"
Name: "{autodesktop}\Fracst"; Filename: "{app}\fracst.exe"; Comment: "Interactive deep-zoom Mandelbrot explorer"; Tasks: desktopicon

[Registry]
; HKA is HKLM for an all-users install and HKCU otherwise.
Root: HKA; Subkey: "Software\Classes\.kfr"; ValueType: string; ValueName: ""; ValueData: "fracst.kfr"; Flags: uninsdeletevalue; Tasks: kfr
Root: HKA; Subkey: "Software\Classes\fracst.kfr"; ValueType: string; ValueName: ""; ValueData: "Fracst location"; Flags: uninsdeletekey; Tasks: kfr
Root: HKA; Subkey: "Software\Classes\fracst.kfr\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\fracst.exe,0"; Tasks: kfr
Root: HKA; Subkey: "Software\Classes\fracst.kfr\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\fracst.exe"" ""%1"""; Tasks: kfr

[Run]
Filename: "{app}\fracst.exe"; Description: "Launch Fracst"; Flags: nowait postinstall skipifsilent

[Code]
// PATH lives in a different key for all-users and per-user installs.
function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

// The PATH without the entry Dir (compared case-insensitively).
function PathWithout(Path, Dir: String): String;
var
  Rest, Part: String;
  I: Integer;
begin
  Result := '';
  Rest := Path + ';';
  while Rest <> '' do begin
    I := Pos(';', Rest);
    Part := Copy(Rest, 1, I - 1);
    Delete(Rest, 1, I);
    if (Part <> '') and (CompareText(RemoveBackslashUnlessRoot(Part), RemoveBackslashUnlessRoot(Dir)) <> 0) then begin
      if Result <> '' then Result := Result + ';';
      Result := Result + Part;
    end;
  end;
end;

procedure SetPath(Add: Boolean);
var
  Path: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then Path := '';
  Path := PathWithout(Path, ExpandConstant('{app}'));
  if Add then begin
    if Path <> '' then Path := Path + ';';
    Path := Path + ExpandConstant('{app}');
  end;
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then SetPath(True);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then SetPath(False);
end;
