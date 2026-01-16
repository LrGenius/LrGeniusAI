@echo off

echo ====================================
echo Cleaning LrGenius Build Artifacts
echo ====================================
echo.
echo This will remove all build outputs and temporary files.
echo.
@REM pause

echo Cleaning geniusai-server build artifacts...
cd ..\geniusai-server
if exist build rmdir /s /q build
if exist dist rmdir /s /q dist
if exist models\bundle rmdir /s /q models\bundle
cd ..\LrGeniusAI

echo Cleaning LrGeniusAI build artifacts...
if exist build\LrGeniusAI.lrplugin rmdir /s /q build\LrGeniusAI.lrplugin
if exist build rmdir /s /q build
if exist dist rmdir /s /q dist

echo.
echo ====================================
echo Clean complete!
echo ====================================
echo.
echo Build artifacts have been removed.
echo Model cache in geniusai-server/models/ was preserved.
echo.
echo To rebuild, run: build-all.bat
echo.
@REM pause
