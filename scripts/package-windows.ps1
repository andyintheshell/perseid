<#
  Build a per-machine WiX v5 MSI and a no-install portable ZIP.
  Run after building the native Windows binary with:
    cargo build --release --locked -p perseid

  Required:
    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.UI.wixext/5.0.2

  Usage:
    pwsh scripts/package-windows.ps1 -Arch x64
    pwsh scripts/package-windows.ps1 -Arch arm64
#>
param(
    [ValidateSet('x64', 'arm64')]
    [string] $Arch = 'x64'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Root

$Version = (& python -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])').Trim()
if ($LASTEXITCODE -ne 0 -or -not $Version) {
    throw 'Unable to read the workspace version from Cargo.toml'
}

# MSI ProductVersion is numeric; filenames retain the full Cargo SemVer.
$MsiVersion = ($Version -split '[-+]')[0]
if ($MsiVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw "MSI requires a numeric X.Y.Z version; got '$MsiVersion'"
}

$Bin = Join-Path $Root 'target\release\perseid.exe'
if (-not (Test-Path -LiteralPath $Bin -PathType Leaf)) {
    throw "Missing Rust binary: $Bin"
}

# Check the PE machine and GUI subsystem before producing a labelled package.
$Bytes = [IO.File]::ReadAllBytes($Bin)
if ($Bytes.Length -lt 0x100 -or [Text.Encoding]::ASCII.GetString($Bytes, 0, 2) -ne 'MZ') {
    throw "Not a valid Windows executable: $Bin"
}
$PeOffset = [BitConverter]::ToInt32($Bytes, 0x3C)
if ($PeOffset -lt 0x40 -or $PeOffset + 0x60 -gt $Bytes.Length -or
    [Text.Encoding]::ASCII.GetString($Bytes, $PeOffset, 4) -ne "PE`0`0") {
    throw "Invalid PE header: $Bin"
}
$ActualMachine = [BitConverter]::ToUInt16($Bytes, $PeOffset + 4)
$ExpectedMachine = if ($Arch -eq 'arm64') { 0xAA64 } else { 0x8664 }
if ($ActualMachine -ne $ExpectedMachine) {
    throw ("PE machine mismatch for {0}: found 0x{1:X4}, expected 0x{2:X4}" -f $Arch, $ActualMachine, $ExpectedMachine)
}
$Subsystem = [BitConverter]::ToUInt16($Bytes, $PeOffset + 0x5C)
if ($Subsystem -ne 2) {
    throw "Expected a Windows GUI-subsystem executable (2), found subsystem $Subsystem"
}

$WixCommand = Get-Command wix -ErrorAction SilentlyContinue
if ($null -ne $WixCommand) {
    $WixExe = $WixCommand.Source
} else {
    $WixExe = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
    if (-not (Test-Path -LiteralPath $WixExe -PathType Leaf)) {
        throw 'WiX v5 CLI not found. Install it with dotnet tool install --global wix --version 5.0.2'
    }
}

$Dist = Join-Path $Root 'dist'
$Stage = Join-Path $Dist "staging\windows\$Arch"
$PortableStage = Join-Path $Dist "staging\windows\$Arch-portable"
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
foreach ($Directory in @($Stage, $PortableStage)) {
    if (Test-Path -LiteralPath $Directory) {
        Remove-Item -LiteralPath $Directory -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Directory | Out-Null
}

Copy-Item -LiteralPath $Bin -Destination (Join-Path $Stage 'perseid.exe')
foreach ($License in @('LICENSE-MIT', 'LICENSE-APACHE')) {
    $Path = Join-Path $Root $License
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing license file: $Path"
    }
    Copy-Item -LiteralPath $Path -Destination (Join-Path $Stage $License)
}

$Msi = Join-Path $Dist "perseid-$Version-windows-$Arch.msi"
$Source = Join-Path $Root 'packaging\windows\perseid.wxs'
Write-Output "Building WiX v5 $Arch MSI: $Msi"
& $WixExe build $Source -arch $Arch -ext WixToolset.UI.wixext `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -o $Msi
if ($LASTEXITCODE -ne 0) { throw "WiX MSI build failed ($LASTEXITCODE)" }
if (-not (Test-Path -LiteralPath $Msi -PathType Leaf)) { throw "Missing installer: $Msi" }

$ValidationDir = Join-Path $Dist "staging\windows\$Arch-msi-validation"
New-Item -ItemType Directory -Force -Path $ValidationDir | Out-Null
& $WixExe msi validate $Msi -ice ICE50 -intermediateFolder $ValidationDir
if ($LASTEXITCODE -ne 0) { throw "WiX ICE50 validation failed ($LASTEXITCODE)" }
$WixPdb = [IO.Path]::ChangeExtension($Msi, '.wixpdb')
if (Test-Path -LiteralPath $WixPdb) { Remove-Item -LiteralPath $WixPdb -Force }

# The ZIP contains the application, not an installer. User data stays in the normal
# per-user location, so installing or replacing the application cannot remove it.
foreach ($Filename in @('perseid.exe', 'LICENSE-MIT', 'LICENSE-APACHE')) {
    Copy-Item -LiteralPath (Join-Path $Stage $Filename) -Destination (Join-Path $PortableStage $Filename)
}
Copy-Item -LiteralPath (Join-Path $Root 'README.md') -Destination (Join-Path $PortableStage 'README.md')
$PortableZip = Join-Path $Dist "perseid-$Version-windows-$Arch-portable.zip"
if (Test-Path -LiteralPath $PortableZip) { Remove-Item -LiteralPath $PortableZip -Force }
Compress-Archive -Path (Join-Path $PortableStage '*') -DestinationPath $PortableZip -CompressionLevel Optimal
if (-not (Test-Path -LiteralPath $PortableZip -PathType Leaf)) { throw "Missing portable ZIP: $PortableZip" }

Write-Output "Created $Msi and $PortableZip (unsigned)"
