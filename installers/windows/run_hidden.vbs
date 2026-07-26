' Launches lrgenius-server.exe with no visible console window.
' Used by the Registry "Run" startup entry and by the self-updater's
' relaunch step — both need to background a console-subsystem exe on
' Windows without a separate windowless binary (the Python era used
' pythonw.exe for this; a single Rust binary can't have two subsystems,
' so this tiny wrapper plays that role instead).
'
' Usage: wscript.exe run_hidden.vbs "<path to lrgenius-server.exe>" [args...]
Dim shell, cmd, i
Set shell = CreateObject("WScript.Shell")

cmd = """" & WScript.Arguments(0) & """"
For i = 1 To WScript.Arguments.Count - 1
    cmd = cmd & " """ & WScript.Arguments(i) & """"
Next

' 0 = hidden window style, False = don't wait for it to exit.
shell.Run cmd, 0, False
