# Install SecureModelRoute on Windows x86_64.
param(
    [switch]$Service
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = if (Test-Path (Join-Path $ScriptDir "smr.exe")) { $ScriptDir } else { Split-Path -Parent $ScriptDir }

$Prefix = if ($env:SMR_INSTALL_PREFIX) { $env:SMR_INSTALL_PREFIX } else { Join-Path $env:USERPROFILE ".local" }
$BinDir = Join-Path $Prefix "bin"
$ConfDir = Join-Path $Prefix "etc\securemodelroute"
$SmrExe = Join-Path $BinDir "smr.exe"
$Config = Join-Path $ConfDir "smr.yaml"
$Launcher = Join-Path $BinDir "securemodelroute.cmd"

$SourceExe = Join-Path $Root "smr.exe"
if (-not (Test-Path $SourceExe)) {
    $SourceExe = Join-Path $Root "target\release\smr.exe"
}
if (-not (Test-Path $SourceExe)) {
    Write-Error "smr.exe not found. Run package.ps1 first or extract the release zip."
}

Write-Host "==> Installing to $Prefix"
New-Item -ItemType Directory -Force -Path $BinDir, $ConfDir | Out-Null
Copy-Item $SourceExe $SmrExe -Force

if (-not (Test-Path $Config)) {
    $Example = Join-Path $Root "smr.example.yaml"
    if (-not (Test-Path $Example)) {
        $Example = Join-Path $Root "config\smr.example.yaml"
    }
    Copy-Item $Example $Config -Force
    Write-Host "    Created $Config"
}

@(
    "@echo off",
    "start `"`" `"$SmrExe`" --config `"$Config`" --open %*"
) | Set-Content -Path $Launcher -Encoding ASCII

if ($Service) {
    $TaskName = "SecureModelRoute"
    $Action = New-ScheduledTaskAction -Execute $SmrExe -Argument "--config `"$Config`""
    $Trigger = New-ScheduledTaskTrigger -AtLogOn
    $Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Force | Out-Null
    Write-Host "    Scheduled task installed: $TaskName (runs at logon)"
}

# Add bin dir to user PATH if missing
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    $env:Path = "$env:Path;$BinDir"
    Write-Host "    Added $BinDir to user PATH (restart terminal to apply everywhere)"
}

Write-Host ""
Write-Host "Installed:"
Write-Host "  binary:   $SmrExe"
Write-Host "  launcher: $Launcher"
Write-Host "  config:   $Config"
Write-Host "  GUI:      http://127.0.0.1:8080/ui"
Write-Host ""
Write-Host "Run:  securemodelroute"
Write-Host "Or:   smr.exe --config `"$Config`" --open"
Write-Host ""
Write-Host "Background service: .\install.ps1 -Service"
