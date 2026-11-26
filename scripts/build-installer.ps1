<#
.SYNOPSIS
  Builds Pulse-<version>-setup.exe with NSIS from a built pulse-native.exe.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -Exe C:\build\release\pulse-native.exe -Out C:\build\dist

.EXAMPLE
  # A test installer that uses another name everywhere (folder, shortcut, uninstall key, settings folder),
  # so it never touches a real Pulse install:
  ... -File scripts\build-installer.ps1 -Exe <exe> -Out <folder> -TestName "Pulse Test"

  -Makensis  path to makensis.exe (default: the NSIS that Tauri downloaded, or one on PATH)
#>
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$TestName = "",
    [string]$Makensis = ""
)
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path -LiteralPath $Exe)) { throw "exe not found: $Exe" }
$Exe = (Resolve-Path -LiteralPath $Exe).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path

if (-not $Makensis) {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "tauri\NSIS\makensis.exe"),
        "C:\Program Files (x86)\NSIS\makensis.exe"
    )
    $Makensis = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $Makensis) {
        $cmd = Get-Command makensis.exe -ErrorAction SilentlyContinue
        if ($cmd) { $Makensis = $cmd.Source }
    }
}
if (-not $Makensis -or -not (Test-Path -LiteralPath $Makensis)) { throw "makensis.exe not found; pass -Makensis" }

# The workspace version, from the root Cargo.toml.
$toml = Get-Content -LiteralPath (Join-Path $repo "Cargo.toml") -Raw
if ($toml -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"(\d+\.\d+\.\d+)"') { throw "workspace version not found in Cargo.toml" }
$version = $Matches[1]

$name = if ($TestName) { $TestName } else { "Pulse" }
$outFile = Join-Path $Out "$name-$version-setup.exe"
if (Test-Path -LiteralPath $outFile) { Remove-Item -LiteralPath $outFile }

# NSIS opens file names relative to the script folder or absolute; a UNC repo path is fine for File.
$nsisArgs = @(
    "/V2",
    "/DPRODUCT_NAME=$name",
    "/DVERSION=$version",
    "/DVERSION_4=$version.0",
    "/DEXE_SRC=$Exe",
    "/DOUT_FILE=$outFile",
    "/DREPO_ROOT=$repo",
    (Join-Path $repo "installer\pulse.nsi")
)
& $Makensis @nsisArgs
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }

$size = (Get-Item -LiteralPath $outFile).Length
Write-Output "built $outFile ($size bytes)"
