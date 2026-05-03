[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir = "$HOME\.cpl\bin",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

if ($PSVersionTable.PSEdition -eq "Core" -and -not $IsWindows) {
    throw "install.ps1 only installs Windows release assets. Use install.sh on Linux/macOS."
}

$Repo = "kharkilirov1/cognitive-project-layer"
$Headers = @{ "User-Agent" = "cpl-install.ps1" }

if ($Version -eq "latest") {
    Write-Step "Resolving latest release"
    $Release = Invoke-RestMethod -Headers $Headers -Uri "https://api.github.com/repos/$Repo/releases/latest"
} else {
    $Tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
    Write-Step "Resolving release $Tag"
    $Release = Invoke-RestMethod -Headers $Headers -Uri "https://api.github.com/repos/$Repo/releases/tags/$Tag"
}

$TagName = $Release.tag_name
$AssetName = "cognitive-project-layer-$TagName-windows-x86_64.zip"
$Asset = $Release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
if (-not $Asset) {
    $Available = ($Release.assets | ForEach-Object { $_.name }) -join ", "
    throw "Release asset '$AssetName' was not found. Available assets: $Available"
}

$InstallPath = [System.IO.Path]::GetFullPath($InstallDir)
$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("cpl-install-" + [System.Guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempRoot $AssetName
$ExtractPath = Join-Path $TempRoot "extract"

try {
    New-Item -ItemType Directory -Force $TempRoot, $ExtractPath, $InstallPath | Out-Null

    Write-Step "Downloading $AssetName"
    Invoke-WebRequest -Headers $Headers -Uri $Asset.browser_download_url -OutFile $ArchivePath

    Write-Step "Extracting archive"
    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractPath -Force

    $Cpl = Get-ChildItem -Path $ExtractPath -Filter "cpl.exe" -Recurse | Select-Object -First 1
    $CplMcp = Get-ChildItem -Path $ExtractPath -Filter "cpl-mcp.exe" -Recurse | Select-Object -First 1
    if (-not $Cpl -or -not $CplMcp) {
        throw "Archive did not contain cpl.exe and cpl-mcp.exe"
    }

    Write-Step "Installing to $InstallPath"
    Copy-Item $Cpl.FullName (Join-Path $InstallPath "cpl.exe") -Force
    Copy-Item $CplMcp.FullName (Join-Path $InstallPath "cpl-mcp.exe") -Force

    if (-not $NoPath) {
        $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $PathParts = @()
        if ($UserPath) {
            $PathParts = $UserPath -split ";" | Where-Object { $_ }
        }
        $AlreadyOnPath = $PathParts | Where-Object { $_.TrimEnd("\") -ieq $InstallPath.TrimEnd("\") }
        if (-not $AlreadyOnPath) {
            Write-Step "Adding install directory to user PATH"
            $NewUserPath = if ($UserPath) { "$UserPath;$InstallPath" } else { $InstallPath }
            [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
        }
        if (($env:Path -split ";") -notcontains $InstallPath) {
            $env:Path = "$InstallPath;$env:Path"
        }
    }

    Write-Step "Installed"
    $InstalledCpl = Join-Path $InstallPath "cpl.exe"
    $InstalledCplMcp = Join-Path $InstallPath "cpl-mcp.exe"
    & $InstalledCpl --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "cpl installed; this release does not support --version"
    }
    & $InstalledCplMcp --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "cpl-mcp installed; this release does not support --version"
    }

    Write-Host ""
    Write-Host "Installed binaries:"
    Write-Host "  $InstalledCpl"
    Write-Host "  $InstalledCplMcp"
    if (-not $NoPath) {
        Write-Host ""
        Write-Host "Restart your terminal if 'cpl' is not immediately available on PATH."
    }
} finally {
    if (Test-Path $TempRoot) {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force
    }
}
