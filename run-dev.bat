@echo off
setlocal
set "PROJECT_ROOT=%~dp0"
set "PYTHON_EXE=%PROJECT_ROOT%python\.venv\Scripts\python.exe"

if not exist "%PYTHON_EXE%" (
  echo ERROR: The project Python environment is missing.
  echo.
  echo Create it first with:
  echo   python -m venv python\.venv
  echo   python\.venv\Scripts\python -m pip install -r python\requirements.txt
  echo.
  pause
  exit /b 1
)

"%PYTHON_EXE%" -c "import flask, pandas, ydata_profiling" >nul 2>&1
if errorlevel 1 (
  echo ERROR: CSV profiler dependencies are not installed in python\.venv.
  echo.
  echo Install them with:
  echo   python\.venv\Scripts\python -m pip install -r python\requirements.txt
  echo.
  pause
  exit /b 1
)

set "CARGO_RUN=cargo run"
where link >nul 2>&1
if errorlevel 1 (
  rustup toolchain list 2>nul | findstr /b /c:"stable-x86_64-pc-windows-gnu" >nul
  if not errorlevel 1 (
    where gcc >nul 2>&1
    if not errorlevel 1 set "CARGO_RUN=cargo +stable-x86_64-pc-windows-gnu run"
  )
)

echo Starting CSV Profiler on http://localhost:5000
rem START sets the working directory, avoiding nested CMD executable quotes.
start "CSV Profiler :5000" /d "%PROJECT_ROOT%" cmd /d /k python\.venv\Scripts\python.exe python\profiler_server.py

echo Starting Rust OTA Server on http://localhost:7000
start "OTA Server :7000" /d "%PROJECT_ROOT%ota-server" cmd /d /k %CARGO_RUN%

echo Waiting for the CSV Profiler...
set /a WAIT_COUNT=0
:wait_dashboard
curl.exe --silent --fail --max-time 2 http://127.0.0.1:5000/ >nul 2>&1
if not errorlevel 1 goto wait_ota_init
set /a WAIT_COUNT+=1
if %WAIT_COUNT% GEQ 60 goto dashboard_failed
timeout /t 1 /nobreak >nul
goto wait_dashboard

:wait_ota_init
echo Waiting for the Rust OTA API...
set /a WAIT_COUNT=0
:wait_ota
curl.exe --silent --fail --max-time 2 http://127.0.0.1:7000/api/ota/status >nul 2>&1
if not errorlevel 1 goto services_ready
set /a WAIT_COUNT+=1
if %WAIT_COUNT% GEQ 180 goto ota_failed
timeout /t 1 /nobreak >nul
goto wait_ota

:services_ready
echo Both services are ready.
start "" "http://localhost:5000"
goto done

:dashboard_failed
echo ERROR: CSV Profiler did not become ready on port 5000.
echo Review the "CSV Profiler :5000" terminal for the startup error.
goto failed

:ota_failed
echo ERROR: Rust OTA API did not become ready on port 7000.
echo Review the "OTA Server :7000" terminal for the startup error.
goto failed

:failed
pause
exit /b 1

:done
echo Both development servers were opened in separate terminals.
echo Configure OTA_PUBLIC_BASE_URL in .env before deploying to a device.
endlocal
