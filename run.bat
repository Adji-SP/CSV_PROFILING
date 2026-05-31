@echo off
echo Starting CSV Profiler Server...
echo Open http://localhost:5000 in your browser
echo Press Ctrl+C to stop the server.
echo.
start "" "http://localhost:5000"
d:\PropertiesProject-D\ELKA\GUI-SNN\python\.venv\Scripts\python.exe d:\PropertiesProject-D\ELKA\GUI-SNN\python\profiler_server.py
pause
