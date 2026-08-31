# zotero-cli bootstrap installer for Windows (PowerShell 5.1 & 7+)
# https://github.com/ntluong95/zotero-rust-cli
#
# Usage:
#   Invoke-WebRequest https://raw.githubusercontent.com/ntluong95/zotero-rust-cli/main/scripts/install.ps1 -OutFile "$env:TEMP\install-zotero-cli.ps1"
#   & "$env:TEMP\install-zotero-cli.ps1"
#
# Parameters:
#   -InstallDir  Custom install directory (default: %LOCALAPPDATA%\Programs\zotero-cli)
#   -Version     Release version to install (default: latest stable)
#   -Repo        GitHub repository (default: ntluong95/zotero-rust-cli)
#   -AddToPath   Add the install directory to user PATH environment variable

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$InstallDir = $env:ZOTERO_CLI_INSTALL_DIR,
    [string]$Version = $env:ZOTERO_CLI_VERSION,
    [string]$Repo = $(if ($env:ZOTERO_CLI_REPO) { $env:ZOTERO_CLI_REPO } else { "ntluong95/zotero-rust-cli" }),
    [string]$BaseUrl = $env:ZOTERO_CLI_BASE_URL,
    [switch]$AddToPath,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host "Usage: install.ps1 [-InstallDir <path>] [-Version <version>] [-Repo <owner/repo>] [-AddToPath]"
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  -InstallDir <path>   Destination directory (default: %LOCALAPPDATA%\Programs\zotero-cli)"
    Write-Host "  -Version <version>   Version to install (default: latest stable release)"
    Write-Host "  -Repo <owner/repo>   GitHub repository (default: ntluong95/zotero-rust-cli)"
    Write-Host "  -AddToPath           Add install directory to the current User's PATH"
    Write-Host "  -Help                Show this help message"
    exit 0
}

# 1. Determine architecture
$arch = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) {
    $arch = $env:PROCESSOR_ARCHITEW6432
}

if ($arch -ne "AMD64" -and $arch -ne "x86_64") {
    Write-Error "Unsupported architecture: $arch. Only x86_64 / AMD64 is supported on Windows."
    exit 1
}

$target = "x86_64-pc-windows-msvc"

# 2. Determine target install directory
if (-not $InstallDir) {
    $localAppData = $env:LOCALAPPDATA
    if (-not $localAppData) {
        $localAppData = Join-Path $env:USERPROFILE "AppData\Local"
    }
    $InstallDir = Join-Path $localAppData "Programs\zotero-cli"
}

# 3. Determine release base URL
if (-not $BaseUrl) {
    if ($Version) {
        $tag = if (-not $Version.StartsWith("v") -and $Version -match '^[0-9]') { "v$Version" } else { $Version }
        $BaseUrl = "https://github.com/$Repo/releases/download/$tag"
    } else {
        $BaseUrl = "https://github.com/$Repo/releases/latest/download"
    }
}

# 4. Create temporary directory
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("zotero-cli-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null

try {
    # 5. Download SHA256SUMS
    $shaUrl = "$BaseUrl/SHA256SUMS"
    $shaFile = Join-Path $tempDir "SHA256SUMS"
    try {
        Invoke-WebRequest -Uri $shaUrl -OutFile $shaFile -UseBasicParsing
    } catch {
        Write-Error "Failed to download SHA256SUMS from $shaUrl : $_"
        exit 1
    }

    # 6. Download ZIP archive (try alias first, then versioned if specified)
    $aliasName = "zotero-cli-$target.zip"
    $versionedName = ""
    if ($Version) {
        $vTag = if (-not $Version.StartsWith("v") -and $Version -match '^[0-9]') { "v$Version" } else { $Version }
        $versionedName = "zotero-cli-$vTag-$target.zip"
    }

    $zipPath = Join-Path $tempDir $aliasName
    $matchName = $aliasName

    $downloadSuccess = $false
    try {
        Invoke-WebRequest -Uri "$BaseUrl/$aliasName" -OutFile $zipPath -UseBasicParsing
        $downloadSuccess = $true
    } catch {
        if ($versionedName) {
            $zipPath = Join-Path $tempDir $versionedName
            $matchName = $versionedName
            try {
                Invoke-WebRequest -Uri "$BaseUrl/$versionedName" -OutFile $zipPath -UseBasicParsing
                $downloadSuccess = $true
            } catch {
                $downloadSuccess = $false
            }
        }
    }

    if (-not $downloadSuccess) {
        Write-Error "Failed to download release archive for $target from $BaseUrl"
        exit 1
    }

    # 7. Verify SHA-256 checksum
    $shaContent = Get-Content -Path $shaFile
    $expectedHash = $null

    foreach ($line in $shaContent) {
        $parts = -split $line
        if ($parts.Length -ge 2) {
            $hash = $parts[0].Trim().ToLower()
            $file = $parts[1].Trim().TrimStart('*')
            $fileName = [System.IO.Path]::GetFileName($file)
            if ($fileName -eq $matchName -or ($versionedName -and $fileName -eq $versionedName)) {
                $expectedHash = $hash
                break
            }
        }
    }

    if (-not $expectedHash) {
        Write-Error "Checksum for $matchName not found in SHA256SUMS."
        exit 1
    }

    $actualHash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()

    if ($actualHash -ne $expectedHash) {
        Write-Error "Checksum verification failed for $matchName`n  Expected: $expectedHash`n  Actual:   $actualHash"
        exit 1
    }

    # 8. Extract archive
    $extractDir = Join-Path $tempDir "extracted"
    New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

    $srcExe = (Get-ChildItem -Path $extractDir -Filter "zotero-cli.exe" -Recurse | Select-Object -First 1)
    if (-not $srcExe -or -not (Test-Path $srcExe.FullName)) {
        Write-Error "Could not locate zotero-cli.exe executable in the extracted archive."
        exit 1
    }

    # 9. Install binary to destination directory
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $destExe = Join-Path $InstallDir "zotero-cli.exe"
    Copy-Item -Path $srcExe.FullName -Destination $destExe -Force

    # Also copy cli-anything-zotero.exe alias if present
    $srcAlias = (Get-ChildItem -Path $extractDir -Filter "cli-anything-zotero.exe" -Recurse | Select-Object -First 1)
    if ($srcAlias -and (Test-Path $srcAlias.FullName)) {
        $destAlias = Join-Path $InstallDir "cli-anything-zotero.exe"
        Copy-Item -Path $srcAlias.FullName -Destination $destAlias -Force
    }

    # 10. Verify execution
    try {
        $installedVersion = (& "$destExe" --version 2>$null)
        if (-not $installedVersion) {
            $installedVersion = "zotero-cli"
        }
    } catch {
        Write-Error "Installed binary failed to execute at $destExe : $_"
        exit 1
    }

    # 11. Check PATH & report
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $currentPath = $env:PATH
    $inPath = $false

    if ($currentPath) {
        $pathEntries = $currentPath -split ';'
        if ($pathEntries -contains $InstallDir) {
            $inPath = $true
        }
    }
    if (-not $inPath -and $userPath) {
        $userEntries = $userPath -split ';'
        if ($userEntries -contains $InstallDir) {
            $inPath = $true
        }
    }

    if ($AddToPath -and -not $inPath) {
        $newUserPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
        $env:PATH = "$env:PATH;$InstallDir"
        $inPath = $true
    }

    Write-Host "$installedVersion installed successfully"
    Write-Host "Path: $destExe"
    Write-Host ""

    if (-not $inPath) {
        Write-Host "Add this directory to your user PATH:"
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$InstallDir`", 'User')"
        Write-Host ""
    }

    Write-Host "Next:"
    Write-Host "  zotero-cli --json app doctor"
}
finally {
    if (Test-Path $tempDir) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
