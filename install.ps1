# rust-hf-downloader installer / upgrader for Windows (PowerShell 5.1+).
#
# One-liner (installs the newest release, or upgrades an existing install):
#   irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
#
# With options (irm | iex cannot pass parameters, so use this form):
#   & ([scriptblock]::Create((irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1))) -Version v2.8.0
#
# Equivalent env overrides: RHD_VERSION, RHD_INSTALL_DIR, RHD_DOWNLOAD_BASE.
# RHD_DOWNLOAD_BASE replaces the GitHub release base URL entirely - useful
# for mirrors and testing.
#
# Design notes:
# - Uses stable asset names published by CI; releases/latest/download/<asset>
#   is a GitHub CDN redirect, so no api.github.com call and no rate limit.
# - Verifies the SHA256 checksum from the same release's SHA256SUMS.
# - No admin rights: installs to %LOCALAPPDATA%\Programs\rust-hf-downloader
#   and updates the per-user PATH.
# - Upgrades handle a running binary: the old exe is renamed aside before the
#   new one is moved in (Windows locks running executables).
[CmdletBinding()]
param(
    [string]$Version = $env:RHD_VERSION,
    [string]$InstallDir = $env:RHD_INSTALL_DIR,
    [switch]$Force,
    [switch]$DryRun,
    [switch]$Uninstall,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
$Repo = 'JohannesBertens/rust-hf-downloader'
$BinName = 'rust-hf-downloader.exe'

if ($Help) {
    Write-Host @'
rust-hf-downloader installer (Windows)

Usage:
  irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
  & ([scriptblock]::Create((irm .../install.ps1))) -Version v2.8.0

Options:
  -Version vX.Y.Z   Install a specific release tag (default: newest)
  -InstallDir DIR   Installation directory (default: %LOCALAPPDATA%\Programs\rust-hf-downloader)
  -Force            Reinstall even if the same version is present
  -DryRun           Print what would be done and exit
  -Uninstall        Remove the installed binary and PATH entry

Environment:
  RHD_VERSION, RHD_INSTALL_DIR, RHD_DOWNLOAD_BASE (override release base URL)
'@
    return
}

function Fail([string]$Message) { throw "install.ps1: $Message" }

# Windows PowerShell 5.1 defaults to TLS 1.0; GitHub requires TLS 1.2+.
try {
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

# Invoke-WebRequest's progress bar makes PS 5.1 downloads an order of
# magnitude slower; silence it for this process.
$ProgressPreference = 'SilentlyContinue'

# --- Platform detection ---------------------------------------------------------
switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { $Triple = 'x86_64-pc-windows-msvc' }
    default { Fail "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE) (only x64 builds are published)" }
}
$Asset = "rust-hf-downloader-$Triple.zip"

# --- Resolve the release download base --------------------------------------------
if ($env:RHD_DOWNLOAD_BASE) {
    $Base = $env:RHD_DOWNLOAD_BASE
} elseif ($Version) {
    if ($Version -notmatch '^v') { $Version = "v$Version" }
    $Base = "https://github.com/$Repo/releases/download/$Version"
} else {
    $Base = "https://github.com/$Repo/releases/latest/download"
}

if (-not $InstallDir) {
    # LOCALAPPDATA is always set on Windows; fall back for non-Windows pwsh
    # (test/mirror environments).
    $AppBase = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } elseif ($env:HOME) { $env:HOME } else { '.' }
    $InstallDir = Join-Path $AppBase 'Programs\rust-hf-downloader'
}
$Dest = Join-Path $InstallDir $BinName

# --- PATH helpers (user scope) -------------------------------------------------------
function Get-UserPathEntries {
    (@([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') |
        Where-Object { $_ -and $_.Trim() } |
        ForEach-Object { $_.Trim().TrimEnd('\') })
}
function Remove-FromUserPath([string]$Dir) {
    $trimmed = $Dir.TrimEnd('\')
    try {
        $entries = @(Get-UserPathEntries | Where-Object { $_ -ne $trimmed })
        [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
        Write-Host "removed $Dir from your user PATH"
    } catch {
        Write-Warning "could not update user PATH: $_"
    }
}

# --- Uninstall ----------------------------------------------------------------------
if ($Uninstall) {
    if (Test-Path -LiteralPath $Dest) {
        if ($DryRun) { Write-Host "dry-run: would remove $Dest"; return }
        Remove-Item -LiteralPath $Dest -Force
        Write-Host "removed $Dest"
        Remove-FromUserPath $InstallDir
    } else {
        Write-Host "nothing to uninstall: $Dest does not exist"
    }
    return
}

Write-Host "plan: install $BinName ($Triple)"
Write-Host "  from: $Base"
Write-Host "  to:   $Dest"
if ($DryRun) { Write-Host 'dry-run: stopping before any download'; return }

# --- Download + verify ------------------------------------------------------------------
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("rhd-install-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    $Sums = Invoke-RestMethod -Uri "$Base/SHA256SUMS" -UseBasicParsing
    $Want = $null
    foreach ($line in ($Sums -split "`n")) {
        $parts = $line -split '\s+', 2
        if ($parts.Count -eq 2 -and ($parts[1].Trim() -eq $Asset -or $parts[1].Trim() -eq "*$Asset")) {
            $Want = $parts[0].ToLower(); break
        }
    }
    if (-not $Want) { Fail "SHA256SUMS at $Base has no entry for $Asset" }

    $ZipPath = Join-Path $Tmp $Asset
    Invoke-WebRequest -Uri "$Base/$Asset" -OutFile $ZipPath -UseBasicParsing

    $Got = (Get-FileHash -Algorithm SHA256 -LiteralPath $ZipPath).Hash.ToLower()
    if ($Got -ne $Want) { Fail "checksum mismatch for $Asset`n  expected: $Want`n  actual:   $Got" }
    Write-Host "checksum ok ($Want)"

    Expand-Archive -Path $ZipPath -DestinationPath $Tmp -Force
    $Exe = Join-Path $Tmp $BinName
    if (-not (Test-Path -LiteralPath $Exe)) { Fail "archive did not contain $BinName" }

    # --- Upgrade check -----------------------------------------------------------------
    $NewVer = & $Exe --version
    if (Test-Path -LiteralPath $Dest) {
        $OldVer = & $Dest --version
        if ($NewVer -and $OldVer -and ($OldVer -eq $NewVer) -and -not $Force) {
            Write-Host "already up to date: $OldVer at $Dest (use -Force to reinstall)"
            return
        }
        if ($OldVer -and $NewVer) { Write-Host "upgrading: $OldVer -> $NewVer" }
    }

    # --- Install --------------------------------------------------------------------------
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    # Windows locks a running exe: rename the old one aside, then move the new
    # one in. If the old exe is still locked, the rename fails with a clear
    # error instead of a half-written binary.
    if (Test-Path -LiteralPath $Dest) {
        Move-Item -LiteralPath $Dest -Destination "$Dest.old" -Force
    }
    Move-Item -LiteralPath $Exe -Destination $Dest -Force
    Remove-Item -LiteralPath "$Dest.old" -Force -ErrorAction SilentlyContinue

    $TrimmedDir = $InstallDir.TrimEnd('\')
    if (@(Get-UserPathEntries) -notcontains $TrimmedDir) {
        try {
            $UserPath = [string][Environment]::GetEnvironmentVariable('Path', 'User')
            $NewUserPath = (@($UserPath -split ';' | Where-Object { $_ }) + $TrimmedDir) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $NewUserPath, 'User')
            Write-Host "added $TrimmedDir to your user PATH (takes effect in new terminals)"
        } catch {
            Write-Warning "could not update user PATH: $_"
        }
    }
    if (($env:Path -split ';' | ForEach-Object { $_.TrimEnd('\') }) -notcontains $TrimmedDir) {
        $env:Path += ";$TrimmedDir" # make it usable in this session too
    }

    Write-Host "installed: $Dest"
    & $Dest --version
    Write-Host "done - run 'rust-hf-downloader' to start (re-run this one-liner any time to upgrade)"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
