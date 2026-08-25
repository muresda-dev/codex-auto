#ifndef AppVersion
  #define AppVersion "0.0.0-auto"
#endif

#ifndef BuildDir
  #error BuildDir must be provided with /DBuildDir=...
#endif

#ifndef OutputDir
  #define OutputDir "."
#endif

#define AppName "Codex Auto"
#define AppPublisher "muresda-dev"
#define AppURL "https://github.com/muresda-dev/codex-auto"

[Setup]
AppId={{8E1318AF-743E-4BA8-BA80-8BA55BFFAD97}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
DefaultDirName={autopf}\Codex Auto
DefaultGroupName=Codex Auto
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\codex.exe
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Codex-Auto-Setup-x64
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "modifypath"; Description: "Добавить Codex Auto в системный PATH"; GroupDescription: "Интеграция:"; Flags: checkedonce

[Files]
Source: "{#BuildDir}\codex.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\codex-code-mode-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\codex-responses-api-proxy.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\codex-windows-sandbox-setup.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\codex-command-runner.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\codex-app-server.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Codex Auto"; Filename: "{app}\codex.exe"

[Code]
const
  EnvironmentKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

procedure AddInstallDirToPath;
var
  CurrentPath: String;
  InstallDir: String;
  Haystack: String;
begin
  if not WizardIsTaskSelected('modifypath') then
    Exit;

  InstallDir := ExpandConstant('{app}');

  if not RegQueryStringValue(HKEY_LOCAL_MACHINE, EnvironmentKey, 'Path', CurrentPath) then
    CurrentPath := '';

  Haystack := ';' + Uppercase(CurrentPath) + ';';
  if Pos(';' + Uppercase(InstallDir) + ';', Haystack) = 0 then
  begin
    if (CurrentPath <> '') and (CurrentPath[Length(CurrentPath)] <> ';') then
      CurrentPath := CurrentPath + ';';

    CurrentPath := CurrentPath + InstallDir;
    RegWriteExpandStringValue(HKEY_LOCAL_MACHINE, EnvironmentKey, 'Path', CurrentPath);
  end;
end;

procedure RemoveInstallDirFromPath;
var
  CurrentPath: String;
  InstallDir: String;
begin
  InstallDir := ExpandConstant('{app}');

  if not RegQueryStringValue(HKEY_LOCAL_MACHINE, EnvironmentKey, 'Path', CurrentPath) then
    Exit;

  if CompareText(CurrentPath, InstallDir) = 0 then
    CurrentPath := ''
  else
  begin
    StringChangeEx(CurrentPath, ';' + InstallDir, '', True);
    StringChangeEx(CurrentPath, InstallDir + ';', '', True);
  end;

  RegWriteExpandStringValue(HKEY_LOCAL_MACHINE, EnvironmentKey, 'Path', CurrentPath);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddInstallDirToPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveInstallDirFromPath;
end;
