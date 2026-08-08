[Setup]
AppName=LrGeniusAI
AppVersion={#AppVersion}
DefaultDirName={commonpf}\LrGeniusAI
DefaultGroupName=LrGeniusAI
OutputBaseFilename=LrGeniusAI-windows-x64-{#AppVersion}
OutputDir=Output
Compression=lzma
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
SetupIconFile=plugin\LrGeniusAI.lrdevplugin\icon.ico
SourceDir=..\..

[Files]
; Backend: a single native binary, no bundled Python runtime.
Source: "build\lrgenius-server\lrgenius-server.exe"; DestDir: "{app}\backend"; Flags: ignoreversion
; llama.cpp's Vulkan backend is a loadable ggml module, so these must sit
; beside the exe or the server runs CPU-only. skipifsourcedoesntexist keeps
; the installer buildable when the llamacpp feature is off.
Source: "build\lrgenius-server\ggml*.dll"; DestDir: "{app}\backend"; Flags: ignoreversion skipifsourcedoesntexist
Source: "installers\windows\run_hidden.vbs"; DestDir: "{app}\backend"; Flags: ignoreversion

; Plugin files (Global location for Lightroom)
Source: "build\LrGeniusAI.lrplugin\*"; DestDir: "{userappdata}\Adobe\Lightroom\Modules\LrGeniusAI.lrplugin"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\LrGeniusAI Backend"; Filename: "{app}\backend\lrgenius-server.exe"; IconFilename: "plugin\LrGeniusAI.lrdevplugin\icon.ico"

[Registry]
; Run backend at system startup for current user, via the hidden-window
; wrapper (the exe itself is a normal console-subsystem binary so the
; `migrate` CLI subcommand keeps working when run from a terminal).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "LrGeniusAIBackend"; ValueData: """wscript.exe"" ""{app}\backend\run_hidden.vbs"" ""{app}\backend\lrgenius-server.exe"""; Flags: uninsdeletevalue

[Run]
; Start the backend immediately after installation, hidden.
Filename: "wscript.exe"; Parameters: """{app}\backend\run_hidden.vbs"" ""{app}\backend\lrgenius-server.exe"""; Description: "Start LrGeniusAI Backend"; Flags: nowait postinstall skipifsilent runhidden

[UninstallRun]
; Stop existing backend process before uninstalling
Filename: "taskkill"; Parameters: "/F /IM lrgenius-server.exe /T"; Flags: runhidden; RunOnceId: "StopBackend"

[Code]
function InitializeSetup(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  // Try to stop the service before starting setup (for updates)
  Exec('taskkill', '/F /IM lrgenius-server.exe /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
