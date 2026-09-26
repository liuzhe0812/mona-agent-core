@echo off
setlocal
cd /d "%~dp0"
set "PYTHONUTF8=1"
set "PYTHONDONTWRITEBYTECODE=1"
where python >nul 2>nul
if errorlevel 1 (
  echo Python 3.11 or later is required. Install Python and enable PATH.
  pause
  exit /b 2
)
python -c "import sys,sqlite3; sys.exit(0 if sys.version_info >= (3,11) else 1)"
if errorlevel 1 (
  echo Python 3.11 or later with sqlite3 is required.
  pause
  exit /b 2
)
python -m harness serve --open %*
set "RESULT=%ERRORLEVEL%"
if not "%RESULT%"=="0" pause
exit /b %RESULT%
