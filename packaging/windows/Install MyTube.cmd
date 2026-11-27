@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install MyTube.ps1"
if errorlevel 1 (
    echo MyTube installation did not complete.
    pause
    exit /b 1
)
echo MyTube is ready. Open it from the Start menu.
pause
