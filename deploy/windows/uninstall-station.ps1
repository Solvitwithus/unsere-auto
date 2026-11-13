# Removes the station program and its startup task. Keeps C:\Weighbridge\data
# (the local database) unless you delete it yourself.
$ErrorActionPreference = "SilentlyContinue"
Stop-ScheduledTask -TaskName "WeighbridgeStation"
Unregister-ScheduledTask -TaskName "WeighbridgeStation" -Confirm:$false
Get-Process wb-station | Stop-Process -Force
Remove-Item "C:\Weighbridge\wb-station.exe", "C:\Weighbridge\wb-scale.exe", "C:\Weighbridge\web" -Recurse -Force
Remove-Item "$([Environment]::GetFolderPath('CommonDesktopDirectory'))\Weighbridge.lnk" -Force
Write-Host "Removed. Data kept in C:\Weighbridge\data."
