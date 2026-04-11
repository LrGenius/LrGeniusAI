[Setup]
AppName=LrGeniusAI
AppVersion={#AppVersion}
DefaultDirName={commonpf}\LrGeniusAI
DefaultGroupName=LrGeniusAI
OutputBaseFilename=LrGeniusAI-windows-x64-{#AppVersion}
Compression=lzma
SolidCompression=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=admin
SetupIconFile=plugin\LrGeniusAI.lrdevplugin\icon.ico

[Files]
; Backend files
Source: "build\lrgenius-server\*"; DestDir: "{app}\backend"; Flags: ignoreversion recursesubdirs createallsubdirs

; Plugin files (Global location for Lightroom)
Source: "build\LrGeniusAI.lrplugin\*"; DestDir: "{commonappdata}\Adobe\Lightroom\Modules\LrGeniusAI.lrplugin"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\LrGeniusAI Backend"; Filename: "{app}\backend\lrgenius-server.cmd"; IconFilename: "{app}\backend\app\src\icon.ico"

[Registry]
; Run backend at system startup for all users
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "LrGeniusAIBackend"; ValueData: """{app}\backend\lrgenius-server.cmd"""; Flags: uninsdeletevalue

[Run]
; Start the backend immediately after installation
Filename: "{app}\backend\lrgenius-server.cmd"; Description: "Start LrGeniusAI Backend"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Stop existing backend process before uninstalling
Filename: "taskkill"; Parameters: "/F /IM python.exe /T /FI ""WINDOWTITLE eq lrgenius-server*"""; Flags: runhidden; RunOnceId: "StopBackend"
Filename: "taskkill"; Parameters: "/F /IM cmd.exe /T /FI ""WINDOWTITLE eq lrgenius-server*"""; Flags: runhidden; RunOnceId: "StopBackendCmd"

[Code]
function InitializeSetup(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  // Try to stop the service/process before starting setup (for updates)
  Exec('taskkill', '/F /IM python.exe /T /FI "WINDOWTITLE eq lrgenius-server*"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec('taskkill', '/F /IM cmd.exe /T /FI "WINDOWTITLE eq lrgenius-server*"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
