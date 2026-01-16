@echo off
setlocal enabledelayedexpansion

echo ====================================
echo Building LrGeniusAI Online Repository
echo ====================================
echo.

for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MAJOR" LrGeniusAI.lrdevplugin\Info.lua') do set MAJOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MINOR" LrGeniusAI.lrdevplugin\Info.lua') do set MINOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.REVISION" LrGeniusAI.lrdevplugin\Info.lua') do set REVISION=%%A

set "VERSION=%MAJOR%.%MINOR%.%REVISION%"
echo Version: %VERSION%
echo.

set /p SERVER_VERSION=< ..\geniusai-server\version.txt
echo Server Version: %SERVER_VERSION%
echo.

echo Update version in config files...


REM Extract distribution files
echo Copying distribution files...
powershell -NoProfile -Command "(Get-Content 'installer\config\config_win.xml') -replace '<Version>.*</Version>', '<Version>%VERSION%</Version>' | Set-Content 'installer\config\config_win.xml'"
powershell -NoProfile -Command "(Get-Content 'installer\packages\com.lrgenius.plugin\meta\package.xml') -replace '<Version>.*</Version>', '<Version>%VERSION%</Version>' | Set-Content 'installer\packages\com.lrgenius.plugin\meta\package.xml'"
powershell -NoProfile -Command "(Get-Content 'installer\packages\com.lrgenius.server\meta\package.xml') -replace '<Version>.*</Version>', '<Version>%SERVER_VERSION%</Version>' | Set-Content 'installer\packages\com.lrgenius.server\meta\package.xml'"

REM Extract Lua plugin
echo Copying Lua plugin...
rmdir /s /q "installer\packages\com.lrgenius.plugin\data" 2>nul
mkdir "installer\packages\com.lrgenius.plugin\data\LrGeniusAI.lrplugin"
xcopy /E /I /Y "build\LrGeniusAI.lrplugin" "installer\packages\com.lrgenius.plugin\data\LrGeniusAI.lrplugin"


REM Extract Server
echo Copying Windows server...
rmdir /s /q "installer\packages\com.lrgenius.server\data" 2>nul
mkdir "installer\packages\com.lrgenius.server\data\lrgenius-server"
xcopy /E /I /Y "..\geniusai-server\dist\lrgenius-server" "installer\packages\com.lrgenius.server\data\lrgenius-server"


REM Create repository directory
set REPO_DIR=repository
mkdir "%REPO_DIR%" 2>nul

echo Generating repository...

REM Generate repository using repogen
C:\Qt\Tools\QtInstallerFramework\4.10\bin\repogen.exe --update-new-components -p installer\packages "%REPO_DIR%"

echo "Compressing repository..."
REM Compress repository using 7-Zip
"C:\Program Files\7-Zip\7z.exe" a -tzip "dist\LrGeniusAI_Repository_Windows_%VERSION%.zip" "%REPO_DIR%\*"