@echo off
setlocal enabledelayedexpansion

REM Read version from Info.lua
set "MAJOR="
set "MINOR="
set "REVISION="

for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MAJOR" LrGeniusAI.lrdevplugin\Info.lua') do set MAJOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.MINOR" LrGeniusAI.lrdevplugin\Info.lua') do set MINOR=%%A
for /f "tokens=2 delims== " %%A in ('findstr /R "^Info.REVISION" LrGeniusAI.lrdevplugin\Info.lua') do set REVISION=%%A

set "VERSION=%MAJOR%.%MINOR%.%REVISION%"
echo Version: %VERSION%

REM Remove build directory
if exist build\LrGeniusAI.lrplugin (
    rmdir /s /q build\LrGeniusAI.lrplugin
)
echo Creating build directory
mkdir build\LrGeniusAI.lrplugin
REM Compile Lua files (replace with your Windows Lua compiler)
cd LrGeniusAI.lrdevplugin
for %%F in (*.lua) do (
    ..\compilers\luac.exe -o ..\build\LrGeniusAI.lrplugin\%%F %%F
)

REM Copy translations
copy TranslatedStrings_*.txt ..\build\LrGeniusAI.lrplugin\
cd ..

endlocal
