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
start "" "http://%PHOTO_PICK_BIND%"
echo Starting photo-pick at http://%PHOTO_PICK_BIND%  (close this window to stop)
"%DIR%photo-pick-server.exe"
endlocal
