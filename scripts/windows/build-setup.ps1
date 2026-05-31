# Build single-file SecureModelRoute Setup.exe (IExpress, no GitHub at install time).
param(
    [string]$StageDir = "C:\Users\Public\smr-setup-stage",
    [string]$OutDir = "C:\Users\Public\smr-setup-out",
    [string]$Version = "0.1.0",
    [string]$LogPath = "C:\Users\Public\smr-setup-out\build-setup.log"
)

$ErrorActionPreference = "Stop"

function Write-SetupLog($msg) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg"
    Write-Host $line
    Add-Content -Path $LogPath -Value $line -ErrorAction SilentlyContinue
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Remove-Item $LogPath -Force -ErrorAction SilentlyContinue

$SmrExe = Join-Path $StageDir "smr.exe"
$GuiExe = Join-Path $StageDir "SecureModelRoute.exe"
$InstallPs1 = Join-Path $StageDir "install.ps1"

foreach ($f in @($SmrExe, $GuiExe, $InstallPs1)) {
    if (-not (Test-Path $f)) {
        Write-SetupLog "ERROR: Missing staging file: $f"
        exit 1
    }
}

$SetupName = "SecureModelRoute-$Version-x64-Setup.exe"
$SetupPath = Join-Path $OutDir $SetupName
Remove-Item $SetupPath -Force -ErrorAction SilentlyContinue

@(
    "@echo off",
    "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""%~dp0install.ps1"" -All",
    "exit /b %ERRORLEVEL%"
) | Set-Content -Path (Join-Path $StageDir "setup.cmd") -Encoding ASCII

$files = Get-ChildItem $StageDir -File | Sort-Object Name
$fileDefs = New-Object System.Collections.Generic.List[string]
$sourceRefs = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $files.Count; $i++) {
    $key = "FILE$i"
    [void]$fileDefs.Add("$key=`"$($files[$i].Name)`"")
    [void]$sourceRefs.Add("%$key%=")
}

$StageDirTrail = $StageDir
if (-not $StageDirTrail.EndsWith('\')) {
    $StageDirTrail += '\'
}

$SedPath = Join-Path $OutDir "setup.sed"
$Sed = @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgramWindow=1
HideExtractAnimation=0
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=%InstallPrompt%
DisplayLicense=
FinishMessage=%FinishMessage%
TargetName=%TargetName%
FriendlyName=%FriendlyName%
AppLaunched=%AppLaunched%
PostInstallCmd=<None>
AdminQuietInstCmd=
UserQuietInstCmd=
SourceFiles=SourceFiles
[Strings]
InstallPrompt=Install SecureModelRoute (CLI + service + desktop GUI)?
FinishMessage=SecureModelRoute installed. Run securemodelroute or open SecureModelRoute from the Start menu.
TargetName=$SetupPath
FriendlyName=SecureModelRoute $Version Setup
AppLaunched=cmd.exe /c setup.cmd
$($fileDefs -join "`r`n")
[SourceFiles]
SourceFiles0=$StageDirTrail
[SourceFiles0]
$($sourceRefs -join "`r`n")
"@

Set-Content -Path $SedPath -Value $Sed -Encoding ASCII
Write-SetupLog "Wrote SED: $SedPath ($($files.Count) files)"

# x86 IExpress stub runs on x64 Windows (WOW64) and WoA; ARM64 stub only runs on ARM Windows.
$IExpressX86 = Join-Path $env:Windir "SysWOW64\iexpress.exe"
$IExpressNative = Join-Path $env:Windir "System32\iexpress.exe"
$IExpress = if (Test-Path $IExpressX86) { $IExpressX86 } else { $IExpressNative }
Write-SetupLog "Using IExpress: $IExpress"

Write-SetupLog "==> Building $SetupName via IExpress..."
$p = Start-Process -FilePath $IExpress -ArgumentList @("/N", "/Q", $SedPath) -Wait -PassThru -NoNewWindow
if ($p.ExitCode -ne 0) {
    Write-SetupLog "ERROR: IExpress exit code $($p.ExitCode)"
    exit 1
}
if (-not (Test-Path $SetupPath)) {
    Write-SetupLog "ERROR: Setup not created at $SetupPath"
    exit 1
}

$size = (Get-Item $SetupPath).Length
Write-SetupLog "==> Setup: $SetupPath ($size bytes)"
