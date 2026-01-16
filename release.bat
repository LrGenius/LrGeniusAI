@echo off
setlocal enabledelayedexpansion

REM Get version from Info.lua
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MAJOR" LrGeniusAI.lrdevplugin\Info.lua') do set MAJOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MINOR" LrGeniusAI.lrdevplugin\Info.lua') do set MINOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.REVISION" LrGeniusAI.lrdevplugin\Info.lua') do set REVISION=%%A
set "VERSION=%MAJOR%.%MINOR%.%REVISION%"

set /p SERVER_VERSION=< ..\geniusai-server\version.txt

echo ====================================
echo Releasing LrGeniusAI Version %VERSION%
echo Server Version %SERVER_VERSION%
echo ====================================
echo.

REM Create version.txt
if not exist dist (mkdir dist)
echo %VERSION% > dist\version.txt

REM Run build steps in sequence
call clean-all.bat
call build-all.bat
REM call build-installer.bat
call build-repository.bat