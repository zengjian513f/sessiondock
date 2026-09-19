@echo off
rem Template rendered by deploy/sdtargets/windows.py (ASCII only, CRLF on upload).
rem Line-for-line mirror of the node's restart-session1.cmd with the binary swap and the
rem web mirror inserted between the stop phase and the relaunch. SSH lives in session 0 and
rem cannot start the node there: a one-shot /IT scheduled task asks the desktop explorer to
rem open the Startup shortcut, exactly like a fresh logon launch, so the node lands in the
rem logged-in desktop session outside the task's no-breakaway Job. The stop phase is the
rem node's own process_identity.py, which never touches ptyhost.exe hosts.
chcp 437 >nul 2>&1
setlocal EnableExtensions
set "SD=@SD@"
set "PY=@PY@"
set "LNK=@LNK@"
if not exist "%LNK%" copy /y "%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\SessionDock.lnk" "%LNK%" >nul
echo ===STOP_OLD===
"%PY%" "%SD%\process_identity.py" --include-supervisor
if errorlevel 1 ( echo STOP_FAILED & exit /b 1 )
echo ===SWAP_BINARY===
for %%N in (@BINS@) do call :swap %%N || exit /b 3
echo ===SWAP_WEB===
if not exist "%SD%\web.staging\index.html" goto :launch
robocopy "%SD%\web.staging" "%SD%\web" /MIR /NFL /NDL /NJH /NJS /NP >nul
if errorlevel 8 ( echo WEB_FAILED & exit /b 4 )
rd /s /q "%SD%\web.staging"
echo WEB_MIRRORED
:launch
del /q "%SD%\STOP" 2>nul
del /q "%SD%\supervisor.lock" 2>nul
echo ===LAUNCH_ON_DESKTOP===
schtasks /delete /tn sd-restart-s1 /f >nul 2>&1
schtasks /create /tn sd-restart-s1 /tr "explorer.exe %LNK%" /sc once /st 23:59 /it /f >nul
schtasks /run /tn sd-restart-s1
ping -n 16 127.0.0.1 >nul
schtasks /delete /tn sd-restart-s1 /f >nul 2>&1
echo ===VERIFY===
powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'sessiondock.exe' -or ($_.Name -eq 'pythonw.exe' -and $_.CommandLine -like '*supervise_sessiondock*') } | ForEach-Object { $pp = Get-CimInstance Win32_Process -Filter ('ProcessId=' + $_.ParentProcessId); Write-Output ($_.Name + ' pid=' + $_.ProcessId + ' session=' + $_.SessionId + ' parent=' + $pp.Name) }"
curl -s -o NUL -w "term/list=%%{http_code}\n" -m 5 @TERM_LIST@
"%PY%" "%SD%\start_sessiondock.py" status
echo ===END===
exit /b 0

:swap
rem A running image can be renamed but not overwritten: park the old file under a
rem stamped .prev name (ptyhost.exe may still be mapped by live hosts), then move the
rem new one in. Parked files that are no longer mapped are deleted on the next run.
if not exist "%SD%\bin\%1.new.exe" ( echo NO_NEW %1 & exit /b 0 )
del /q "%SD%\bin\%1.prev-*.exe" >nul 2>&1
set "PREV=%SD%\bin\%1.prev-%RANDOM%.exe"
if exist "%SD%\bin\%1.exe" move /y "%SD%\bin\%1.exe" "%PREV%" >nul
if errorlevel 1 ( echo UNLINK_FAILED %1 & exit /b 3 )
move /y "%SD%\bin\%1.new.exe" "%SD%\bin\%1.exe" >nul
if errorlevel 1 ( echo SWAP_FAILED %1 & exit /b 3 )
del /q "%PREV%" >nul 2>&1
echo SWAPPED %1
exit /b 0
