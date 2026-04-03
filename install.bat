@echo off
chcp 65001 >nul 2>&1
echo ================================
echo   MD Viewer - Install
echo ================================
echo.
set "VIEWER_PATH=%~dp0md-viewer.exe"
if not exist "%VIEWER_PATH%" (
    echo [ERROR] md-viewer.exe not found.
    pause
    exit /b 1
)
echo Register: %VIEWER_PATH%
echo.
reg add "HKCU\Software\Classes\.md" /ve /d "MDViewer.Document" /f >nul 2>&1
reg add "HKCU\Software\Classes\MDViewer.Document" /ve /d "Markdown Document" /f >nul 2>&1
reg add "HKCU\Software\Classes\MDViewer.Document\shell\open\command" /ve /d "\"%VIEWER_PATH%\" \"%%1\"" /f >nul 2>&1
reg add "HKCU\Software\Classes\.markdown" /ve /d "MDViewer.Document" /f >nul 2>&1
echo.
echo [OK] Done! Double-click .md files to open with MD Viewer.
echo To uninstall, run uninstall.bat
pause
