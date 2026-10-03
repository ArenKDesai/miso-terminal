<#
.SYNOPSIS
    Install MISO Terminal for the current user (no administrator rights needed).

.DESCRIPTION
    Run from the unzipped release folder:

        powershell -ExecutionPolicy Bypass -File .\install.ps1            # Start Menu shortcut
        powershell -ExecutionPolicy Bypass -File .\install.ps1 -Desktop   # plus a desktop shortcut
        powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall

    The app goes to %LOCALAPPDATA%\Programs\MISO Terminal. Settings, themes, the
    report cache and the window layout live in %APPDATA%\MISO Terminal and
    %LOCALAPPDATA%\MISO Terminal; uninstalling keeps them.
#>
param(
    [switch]$Uninstall,
    [switch]$Desktop,
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'Programs\MISO Terminal'),
    # For testing: install without touching the Start Menu or desktop.
    [switch]$NoShortcuts
)
$ErrorActionPreference = 'Stop'
$name = 'MISO Terminal'
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) "$name.lnk"
$desktopLink = Join-Path ([Environment]::GetFolderPath('Desktop')) "$name.lnk"

if ($Uninstall) {
    Get-Process miso-terminal -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like "$Destination*" } |
        Stop-Process -Force
    if (-not $NoShortcuts) {
        foreach ($link in $startMenu, $desktopLink) {
            if (Test-Path $link) { Remove-Item $link }
        }
    }
    if (Test-Path $Destination) { Remove-Item $Destination -Recurse -Force }
    Write-Host "Removed $name. Your settings in $env:APPDATA\$name were kept."
    return
}

$exe = Join-Path $PSScriptRoot 'miso-terminal.exe'
if (-not (Test-Path $exe)) { throw "miso-terminal.exe was not found next to install.ps1" }

New-Item -ItemType Directory -Force $Destination | Out-Null
Copy-Item $exe $Destination -Force

# Recorded responses for `--offline`; replaced wholesale so nothing goes stale.
$fixtures = Join-Path $PSScriptRoot 'fixtures'
if (Test-Path $fixtures) {
    $target = Join-Path $Destination 'fixtures'
    if (Test-Path $target) { Remove-Item $target -Recurse -Force }
    Copy-Item $fixtures $target -Recurse
}

# Files from a downloaded zip carry the Mark of the Web; clear it so Windows
# does not warn on every launch of a copy the user chose to install.
Get-ChildItem $Destination -Recurse -File | Unblock-File

if (-not $NoShortcuts) {
    $shell = New-Object -ComObject WScript.Shell
    $links = @($startMenu)
    if ($Desktop) { $links += $desktopLink }
    foreach ($path in $links) {
        $link = $shell.CreateShortcut($path)
        $link.TargetPath = Join-Path $Destination 'miso-terminal.exe'
        $link.WorkingDirectory = $Destination
        $link.Description = 'Information terminal for MISO market data'
        $link.Save()
    }
}
Write-Host "Installed $name to $Destination"
