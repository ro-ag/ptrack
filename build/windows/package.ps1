<#
.SYNOPSIS
  Packages the Windows per-user MSI installer and the portable ZIP.

.DESCRIPTION
  Reads ptrack.exe and p-track.exe from -BinDir and writes, into -OutDir:
    p-track_<version>_windows_<arch>.msi           per-user installer, no admin
    p-track_<version>_windows_<arch>_portable.zip  run from any folder

  The frozen CLI archive ptrack_<version>_windows_<arch>.zip is packaged by
  the release workflow itself; installed updaters validate its exact entries.

  WiX Toolset 3.14 comes from -WixDir, then $env:WIX314_DIR, then a pinned
  download from the official wix3 release whose SHA-256 is verified before
  anything in it runs. Runs on Windows PowerShell 5.1 and PowerShell 7.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][ValidateSet('amd64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$BinDir,
    [Parameter(Mandatory)][string]$OutDir,
    [string]$WixDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WixUrl = 'https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip'
$WixSha256 = '6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31'

if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "version must be canonical stable SemVer: $Version"
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$BinDir = (Resolve-Path $BinDir).Path
foreach ($name in 'ptrack.exe', 'p-track.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $BinDir $name) -PathType Leaf)) {
        throw "missing $name in $BinDir"
    }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$work = Join-Path ([IO.Path]::GetTempPath()) "ptrack-package-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $work | Out-Null

function Resolve-Wix {
    foreach ($candidate in @($WixDir, $env:WIX314_DIR)) {
        if ($candidate -and (Test-Path -LiteralPath (Join-Path $candidate 'candle.exe'))) {
            return (Resolve-Path $candidate).Path
        }
    }
    $archive = Join-Path $work 'wix314-binaries.zip'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $WixUrl -OutFile $archive
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $WixSha256) {
        throw "WiX download digest mismatch: $actual"
    }
    $destination = Join-Path $work 'wix314'
    Expand-Archive -LiteralPath $archive -DestinationPath $destination
    return $destination
}

# WixUI renders the license from RTF; LICENSE is plain text.
function Write-LicenseRtf([string]$Source, [string]$Destination) {
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('{\rtf1\ansi\deff0{\fonttbl{\f0\fnil Segoe UI;}}\fs18 ')
    foreach ($character in (Get-Content -LiteralPath $Source -Raw -Encoding UTF8).ToCharArray()) {
        switch -CaseSensitive ($character) {
            '\' { [void]$builder.Append('\\'); continue }
            '{' { [void]$builder.Append('\{'); continue }
            '}' { [void]$builder.Append('\}'); continue }
            "`r" { continue }
            "`n" { [void]$builder.Append("\par`r`n"); continue }
            default {
                $code = [int]$character
                if ($code -gt 127) {
                    # RTF spells a code unit as a signed 16-bit number.
                    if ($code -gt 32767) {
                        $code -= 65536
                    }
                    [void]$builder.Append("\u$code?")
                } else {
                    [void]$builder.Append($character)
                }
            }
        }
    }
    [void]$builder.Append('}')
    [IO.File]::WriteAllText($Destination, $builder.ToString(), [Text.Encoding]::ASCII)
}

# The dialog art keeps WixUI's layout: a 164 px brand panel on the left of the
# welcome/finish pages and an icon at the right of the banner, leaving the
# text areas white so the dialog text stays readable.
function Write-InstallerBitmap(
    [string]$Destination, [int]$Width, [int]$Height,
    [int]$PanelWidth, [int]$IconSize, [int]$IconX, [int]$IconY
) {
    Add-Type -AssemblyName System.Drawing
    $bitmap = New-Object System.Drawing.Bitmap $Width, $Height, ([Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $icon = [Drawing.Image]::FromFile((Join-Path $repo 'assets\brand\icon-256.png'))
    try {
        $graphics.Clear([Drawing.Color]::White)
        if ($PanelWidth -gt 0) {
            $panel = New-Object Drawing.SolidBrush ([Drawing.ColorTranslator]::FromHtml('#080d12'))
            $graphics.FillRectangle($panel, 0, 0, $PanelWidth, $Height)
            $panel.Dispose()
        }
        $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.DrawImage($icon, $IconX, $IconY, $IconSize, $IconSize)
        $bitmap.Save($Destination, [Drawing.Imaging.ImageFormat]::Bmp)
    } finally {
        $icon.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

# Proves, through Windows Installer itself, that the package is the one this
# script promises: the requested platform, marked as needing no elevation,
# and per-user (no ALLUSERS property to promote it to a machine install).
function Assert-PerUserPackage([string]$Path, [string]$Platform) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $type = $installer.GetType()
    $summary = $type.InvokeMember('SummaryInformation', 'GetProperty', $null, $installer, @($Path, 0))
    $template = $summary.GetType().InvokeMember('Property', 'GetProperty', $null, $summary, @(7))
    $words = [int]$summary.GetType().InvokeMember('Property', 'GetProperty', $null, $summary, @(15))
    if ($template -ne "$Platform;1033") {
        throw "installer platform is '$template', expected '$Platform;1033'"
    }
    if (($words -band 8) -eq 0) {
        throw 'installer is not marked as needing no elevation'
    }
    $database = $type.InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
    $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @(
            "SELECT ``Value`` FROM ``Property`` WHERE ``Property`` = 'ALLUSERS'"))
    $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
    $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
    $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null)
    if ($null -ne $record) {
        throw 'installer sets ALLUSERS, which would make it a machine-wide install'
    }
}

function Invoke-Tool([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$(Split-Path -Leaf $Program) failed with exit code $LASTEXITCODE"
    }
}

try {
    $wix = Resolve-Wix
    $licenseRtf = Join-Path $work 'license.rtf'
    $banner = Join-Path $work 'banner.bmp'
    $dialog = Join-Path $work 'dialog.bmp'
    Write-LicenseRtf (Join-Path $repo 'LICENSE') $licenseRtf
    Write-InstallerBitmap $banner 493 58 0 40 443 9
    Write-InstallerBitmap $dialog 493 312 164 96 34 60

    $platform = if ($Arch -eq 'amd64') { 'x64' } else { 'arm64' }
    $object = Join-Path $work 'p-track.wixobj'
    $msi = Join-Path $OutDir "p-track_${Version}_windows_${Arch}.msi"
    $extensions = @('-ext', 'WixUIExtension', '-ext', 'WixUtilExtension')
    Invoke-Tool (Join-Path $wix 'candle.exe') (@(
            '-nologo', '-arch', $platform,
            "-dVersion=$Version",
            "-dBinDir=$BinDir",
            "-dDocsDir=$repo",
            "-dIconPath=$(Join-Path $repo 'src-tauri\icons\icon.ico')",
            "-dLicenseRtf=$licenseRtf",
            "-dBannerBmp=$banner",
            "-dDialogBmp=$dialog",
            '-out', $object
        ) + $extensions + @((Join-Path $PSScriptRoot 'p-track.wxs')))
    # ICE91 only notes that per-user files cannot be redirected per machine,
    # which is the point of a per-user package.
    Invoke-Tool (Join-Path $wix 'light.exe') (@(
            '-nologo', '-cultures:en-us', '-spdb', '-sice:ICE91',
            '-out', $msi
        ) + $extensions + @($object))
    Assert-PerUserPackage $msi $(if ($Arch -eq 'amd64') { 'x64' } else { 'Arm64' })

    $portable = Join-Path $work 'portable'
    New-Item -ItemType Directory -Path $portable | Out-Null
    Copy-Item -LiteralPath (Join-Path $BinDir 'p-track.exe'), (Join-Path $BinDir 'ptrack.exe'), `
        (Join-Path $repo 'README.md'), (Join-Path $repo 'LICENSE') -Destination $portable
    $zip = Join-Path $OutDir "p-track_${Version}_windows_${Arch}_portable.zip"
    if (Test-Path -LiteralPath $zip) {
        Remove-Item -LiteralPath $zip
    }
    Compress-Archive -Path (Join-Path $portable '*') -DestinationPath $zip
    Get-Item -LiteralPath $msi, $zip | ForEach-Object { "{0}  {1:N0} bytes" -f $_.Name, $_.Length }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
