@echo off
rem Template rendered by deploy/sdtargets/windows.py (ASCII only, CRLF on upload).
rem Copy the live binaries and mirror the web tree into the backup directory.
chcp 437 >nul 2>&1
setlocal EnableExtensions
set "SD=@SD@"
set "BK=@BACKUP@"
if not exist "%BK%\bin" mkdir "%BK%\bin"
for %%N in (@BINS@) do if exist "%SD%\bin\%%N.exe" copy /y "%SD%\bin\%%N.exe" "%BK%\bin\%%N.exe" >nul || exit /b 1
robocopy "%SD%\web" "%BK%\web" /MIR /NFL /NDL /NJH /NJS /NP >nul
if errorlevel 8 ( echo ROBOCOPY_FAILED & exit /b 2 )
echo BACKUP_OK
exit /b 0
