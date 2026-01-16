@echo off
setlocal enabledelayedexpansion

echo ====================================
echo Building LrGenius Application for Windows
echo ====================================
echo.

REM Step 1: Build the geniusai-server
echo [1/4] Building geniusai-server...
cd ..\geniusai-server
call build.bat
if %errorlevel% neq 0 (
    echo ERROR: Failed to build geniusai-server
    exit /b 1
)
echo.

REM Step 3: Build the Lightroom plugin
echo [3/4] Building Lightroom plugin...
cd ..\LrGeniusAI
call build.bat
if %errorlevel% neq 0 (
    echo ERROR: Failed to build Lightroom plugin
    exit /b 1
)
echo.

