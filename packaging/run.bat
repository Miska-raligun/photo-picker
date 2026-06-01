@echo off
REM photo-pick launcher (Windows). Starts the local server (UI embedded in the
REM binary, models bundled alongside) and opens it in your browser.
REM
REM Override the bind address with PHOTO_PICK_BIND, e.g.:
REM   set PHOTO_PICK_BIND=0.0.0.0:7777 && run.bat
setlocal
set "DIR=%~dp0"
if "%PHOTO_PICK_MODELS_DIR%"=="" set "PHOTO_PICK_MODELS_DIR=%DIR%models"
if "%PHOTO_PICK_BIND%"=="" set "PHOTO_PICK_BIND=127.0.0.1:7777"
set "URL=http://%PHOTO_PICK_BIND%"
echo Starting photo-pick at %URL%  (close this window to stop)
REM Open the browser. `start` uses the default URL handler; fall back to
REM rundll32's protocol handler if that fails (rare — broken Windows). If
REM both fail, the URL is echoed above so the user can paste it manually.
start "" "%URL%" 2>nul || rundll32 url.dll,FileProtocolHandler "%URL%" 2>nul || echo (could not auto-open browser; visit %URL% manually)
"%DIR%photo-pick-server.exe"
endlocal
