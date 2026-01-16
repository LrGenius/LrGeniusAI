@echo off
setlocal enabledelayedexpansion

echo ====================================
echo Building LrGeniusAI Installer
echo ====================================
echo.

REM Get version from Info.lua
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MAJOR" LrGeniusAI.lrdevplugin\Info.lua') do set MAJOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MINOR" LrGeniusAI.lrdevplugin\Info.lua') do set MINOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.REVISION" LrGeniusAI.lrdevplugin\Info.lua') do set REVISION=%%A
set "VERSION=%MAJOR%.%MINOR%.%REVISION%"
echo Version: %VERSION%

echo Building installer...

REM Create installer
set INSTALLER_OUTPUT=dist\LrGeniusAI-Installer-Windows.exe

md dist 2>nul

echo Creating online installer ^(requires repository^)...
"C:\Qt\Tools\QtInstallerFramework\4.10\bin\binarycreator.exe" --online-only ^
    -c installer\config\config_win.xml ^
    -p installer\packages ^
    %INSTALLER_OUTPUT%

echo.
echo ====================================
echo Installer created successfully!
echo Output: %INSTALLER_OUTPUT%
echo ====================================
echo.

