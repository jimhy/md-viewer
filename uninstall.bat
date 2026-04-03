@echo off
chcp 65001 >nul 2>&1
echo ================================
echo   MD Viewer - Uninstall
echo ================================
echo.
reg delete "HKCU\Software\Classes\.md" /f >nul 2>&1
reg delete "HKCU\Software\Classes\.markdown" /f >nul 2>&1
reg delete "HKCU\Software\Classes\MDViewer.Document" /f >nul 2>&1
echo [OK] File association removed.
pause
