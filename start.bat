@echo off
setlocal
cd /d "%~dp0"
title DrawCraft - Web & iPadOS Server

if /i "%~1"=="test" (
    echo ==============================================
    echo  Running CI + WASM Web Build Verification
    echo ==============================================
    cargo xtask ci
    if errorlevel 1 (
        echo [FAIL] cargo xtask ci failed!
        pause
        exit /b 1
    )
    cargo xtask web-build
    if errorlevel 1 (
        echo [FAIL] cargo xtask web-build failed!
        pause
        exit /b 1
    )
    echo [OK] All checks and WASM build passed!
    pause
    exit /b 0
)

if /i "%~1"=="build" (
    cargo xtask web-build
    pause
    exit /b %errorlevel%
)

where trunk >nul 2>&1
if errorlevel 1 (
    echo [ERROR] trunk is not installed on PATH.
    echo Run once: cargo install --locked trunk
    pause
    exit /b 1
)

set "SCHEME=http"
set "EXTRA_ARGS="
set "LOCAL_URL=%SCHEME%://localhost:8080"

echo ==============================================
echo  DrawCraft (Pure-Rust WASM PWA)
echo  PC   : %LOCAL_URL% (opens automatically)
echo  iPad : Open the LAN URL printed below in Safari
echo  Press Ctrl+C to stop
echo ==============================================

start "" /b powershell -NoProfile -WindowStyle Hidden -Command "for ($i = 0; $i -lt 240; $i++) { try { $c = New-Object Net.Sockets.TcpClient('127.0.0.1', 8080); $c.Close(); Start-Process '%LOCAL_URL%'; break } catch { Start-Sleep -Milliseconds 500 } }"

cargo xtask dev %EXTRA_ARGS%
pause
