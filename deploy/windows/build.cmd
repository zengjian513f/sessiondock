@echo off
rem Template rendered by deploy/sdtargets/windows.py (ASCII only, CRLF on upload).
rem Extract the source zip into the build directory keeping target, build with the
rem toolchain's real cargo.exe (the rustup shims in .cargo\bin are reparse points that an
rem elevated SSH session cannot execute) and stage the fresh binaries as NAME.new.exe.
rem Each line is parsed on its own, so ERRORLEVEL is the real exit code of the previous line.
chcp 437 >nul 2>&1
setlocal EnableExtensions
set "SD=@SD@"
set "SRC=@SRC@"
set "TC=@TC@"
set "ZIP=@ZIP@"
set "PATH=%TC%;%PATH%"
set "RUSTC=%TC%\rustc.exe"
set "RUSTDOC=%TC%\rustdoc.exe"
if not exist "%TC%\cargo.exe" ( echo NO_CARGO %TC% & exit /b 10 )
if not exist "%ZIP%" ( echo NO_ZIP %ZIP% & exit /b 11 )
if not exist "%SRC%" mkdir "%SRC%"
echo ===EXTRACT===
for /d %%D in ("%SRC%\*") do if /i not "%%~nxD"=="target" if /i not "%%~nxD"==".deploy" rd /s /q "%%D"
del /q "%SRC%\*" >nul 2>&1
powershell -NoProfile -Command "Expand-Archive -LiteralPath '%ZIP%' -DestinationPath '%SRC%' -Force"
if errorlevel 1 ( echo EXTRACT_FAILED & exit /b 12 )
if not exist "%SRC%\Cargo.toml" ( echo NO_CARGO_TOML & exit /b 13 )
del /q "%ZIP%" >nul 2>&1
echo ===TOOLCHAIN===
"%TC%\cargo.exe" --version
if errorlevel 1 ( echo CARGO_BROKEN & exit /b 14 )
"%RUSTC%" --version
if errorlevel 1 ( echo RUSTC_BROKEN & exit /b 15 )
echo ===BUILD===
cd /d "%SRC%"
for %%N in (@BINS@) do call :mtime %%N BEFORE
"%TC%\cargo.exe" build --release --locked @PKGS@ > "%SRC%\.deploy-build.log" 2>&1
set "RC=%ERRORLEVEL%"
powershell -NoProfile -Command "Get-Content -LiteralPath '%SRC%\.deploy-build.log' -Tail 25"
del /q "%SRC%\.deploy-build.log" >nul 2>&1
if not "%RC%"=="0" ( echo BUILD_FAILED rc=%RC% & exit /b 16 )
for %%N in (@BINS@) do call :mtime %%N AFTER
echo ===STAGE===
for %%N in (@BINS@) do call :stage %%N || exit /b 17
echo BUILD_OK
exit /b 0

:mtime
if not exist "%SRC%\target\release\%1.exe" ( echo MTIME_%2 %1 0 & exit /b 0 )
for /f "usebackq delims=" %%T in (`powershell -NoProfile -Command "(Get-Item -LiteralPath '%SRC%\target\release\%1.exe').LastWriteTimeUtc.Ticks"`) do echo MTIME_%2 %1 %%T
exit /b 0

:stage
if not exist "%SRC%\target\release\%1.exe" ( echo NO_EXE %1 & exit /b 1 )
copy /y "%SRC%\target\release\%1.exe" "%SD%\bin\%1.new.exe" >nul
if errorlevel 1 ( echo COPY_FAILED %1 & exit /b 1 )
for /f "skip=1 tokens=*" %%H in ('certutil -hashfile "%SD%\bin\%1.new.exe" SHA256 ^| findstr /v /i "certutil"') do ( echo NEW_SHA %1 %%H & exit /b 0 )
echo HASH_FAILED %1
exit /b 1
