# Installs the weighbridge station on a Windows PC.
#
# Run in an *Administrator* PowerShell from the folder that contains
# wb-station.exe and the "web" folder:
#     Set-ExecutionPolicy -Scope Process Bypass
#     .\install-station.ps1
#
# What it does:
#   1. Copies the program to C:\Weighbridge (data stays in C:\Weighbridge\data)
#   2. Registers a scheduled task that starts the station at boot and restarts it
#      if it stops (runs as SYSTEM, so it works before anyone logs in)
#   3. Puts a "Weighbridge" shortcut on the desktop that opens the operator
#      screen in its own window (Microsoft Edge app mode)
#   4. Opens the screen so you can do the setup straight away

$ErrorActionPreference = "Stop"
$Target = "C:\Weighbridge"
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "Installing to $Target ..."
New-Item -ItemType Directory -Force -Path $Target, "$Target\data" | Out-Null
# stop a running copy before overwriting it (upgrades)
Get-ScheduledTask -TaskName "WeighbridgeStation" -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue
Get-Process wb-station -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Copy-Item "$Here\wb-station.exe" "$Target\" -Force
if (Test-Path "$Here\wb-scale.exe") { Copy-Item "$Here\wb-scale.exe" "$Target\" -Force }
Copy-Item "$Here\web" "$Target\" -Recurse -Force

$action = New-ScheduledTaskAction -Execute "$Target\wb-station.exe" `
  -Argument "--data-dir `"$Target\data`" --web-dir `"$Target\web`"" -WorkingDirectory $Target
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
Register-ScheduledTask -TaskName "WeighbridgeStation" -Action $action -Trigger $trigger -Settings $settings `
  -User "SYSTEM" -RunLevel Highest -Force | Out-Null
Start-ScheduledTask -TaskName "WeighbridgeStation"

$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
if (-not (Test-Path $edge)) { $edge = "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe" }
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut("$([Environment]::GetFolderPath('CommonDesktopDirectory'))\Weighbridge.lnk")
$lnk.TargetPath = $edge
$lnk.Arguments = "--app=http://127.0.0.1:7878/ --start-maximized"
$lnk.IconLocation = "$Target\wb-station.exe,0"
$lnk.Save()

Start-Sleep -Seconds 3
Start-Process $edge "--app=http://127.0.0.1:7878/ --start-maximized"
Write-Host "Done. The station runs in the background and starts with Windows."
Write-Host "Use the 'Weighbridge' icon on the desktop to open it."
