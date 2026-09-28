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
# - Cargo-takeover: if a 'cargo install rust-hf-downloader' copy exists in
#   $CARGO_HOME\bin (%USERPROFILE%\.cargo\bin), the installer upgrades it IN
#   PLACE, handing 'cargo uninstall' over first so cargo's install records
#   stay clean. Otherwise, if that dir exists and is on PATH it is preferred
#   (cargo-binstall convention) so cargo copies cannot shadow release copies.
# - No admin rights by default: installs to
#   %LOCALAPPDATA%\Programs\rust-hf-downloader and updates the per-user PATH.
# - Upgrades handle a running binary: the old exe is renamed aside before the
#   new one is moved in (Windows locks running executables).
# - After installing, re-resolves the command the way the shell would (first
#   PATH match) and warns if a different copy shadows the new one.
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

# --- Default install dir: cargo takeover / cargo-binstall convention --------------
# Env vars are case-sensitive on Unix: the process PATH variable is 'PATH'
# there and 'Path' on Windows. Resolve once, use everywhere.
$ProcPathVar = if ([Environment]::GetEnvironmentVariable('Path', 'Process')) { 'Path' } else { 'PATH' }
function Get-ProcPath { [Environment]::GetEnvironmentVariable($Script:ProcPathVar, 'Process') }
$CargoHomeBase = if ($env:USERPROFILE) { $env:USERPROFILE } elseif ($env:HOME) { $env:HOME } else { $null }
$CargoBin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } elseif ($CargoHomeBase) { Join-Path $CargoHomeBase '.cargo\bin' } else { $null }
$CargoBinTrimmed = if ($CargoBin) { $CargoBin.TrimEnd('\') } else { $null }
$ProcPathSep = [IO.Path]::PathSeparator
$CargoBinOnPath = $CargoBinTrimmed -and (@((Get-ProcPath) -split $ProcPathSep | ForEach-Object { $_.Trim().TrimEnd('\') } | Where-Object { $_ }) -contains $CargoBinTrimmed)
$TakeoverCargo = $false
if (-not $InstallDir) {
    if ($CargoBinTrimmed -and (Test-Path -LiteralPath (Join-Path $CargoBin $BinName))) {
        # A 'cargo install rust-hf-downloader' copy lives here: upgrade it in
        # place so there stays exactly one binary, where PATH already points.
        $InstallDir = $CargoBin
        $TakeoverCargo = $true
    } elseif ($CargoBinTrimmed -and (Test-Path -LiteralPath $CargoBin) -and $CargoBinOnPath) {
        # cargo-binstall convention: Rust CLI binaries live in $CARGO_HOME\bin.
        $InstallDir = $CargoBin
    } else {
        # LOCALAPPDATA is always set on Windows; fall back for non-Windows pwsh
        # (test/mirror environments).
        $AppBase = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } elseif ($env:HOME) { $env:HOME } else { '.' }
        $InstallDir = Join-Path $AppBase 'Programs\rust-hf-downloader'
    }
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
if ($TakeoverCargo) { Write-Host "  note: taking over the cargo-installed copy in $CargoBin (in-place upgrade)" }if ($DryRun) { Write-Host 'dry-run: stopping before any download'; return }

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
    if ($TakeoverCargo) {
        # Let cargo forget its install record via the sanctioned path. Best
        # effort: if cargo is missing or the exe is locked by a running
        # process, we simply replace the file below anyway.
        if (Get-Command cargo -ErrorAction SilentlyContinue) {
            Write-Host "running 'cargo uninstall rust-hf-downloader' to release cargo's install record"
            # Relax EAP around the native call: on PS 5.1, redirected stderr
            # + Stop would turn cargo's progress chatter into an exception.
            $PrevEap = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { & cargo uninstall rust-hf-downloader 2>$null | Out-Null } catch { }
            $ErrorActionPreference = $PrevEap
        } else {
            Write-Warning 'cargo not found on PATH - replacing the binary without cleaning cargo''s install record'
        }
    }
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
    if ((@((Get-ProcPath) -split $ProcPathSep | ForEach-Object { $_.TrimEnd('\') })) -notcontains $TrimmedDir) {
        [Environment]::SetEnvironmentVariable($Script:ProcPathVar, "$(Get-ProcPath)$ProcPathSep$TrimmedDir", 'Process') # usable in this session too
    }

    Write-Host "installed: $Dest"
    & $Dest --version

    # --- Volta-style shadow check: what will the shell actually resolve? ----------------
    # Manual first-match scan of PATH (don't rely on Get-Command: its PATH
    # cache does not reliably refresh after runtime PATH updates).
    $Resolved = $null
    foreach ($dir in (@((Get-ProcPath) -split $ProcPathSep | ForEach-Object { $_.Trim() } | Where-Object { $_ }))) {
        $Candidate = Join-Path $dir $BinName
        if (Test-Path -LiteralPath $Candidate) { $Resolved = $Candidate; break }
    }
    if ($Resolved -and ($Resolved -ine $Dest)) {
        Write-Warning @"
another $BinName is earlier on your PATH:
  $Resolved
shadows the newly installed $Dest - typing '$BinName' will run the OTHER one.
Remove the old copy (e.g. 'cargo uninstall rust-hf-downloader',
'scoop uninstall ...', 'winget uninstall ...') or reorder your PATH so
$InstallDir comes first.
"@
    } elseif (-not $Resolved) {
        Write-Host "NOTE: $InstallDir is not on your PATH; add it (System Settings > Environment Variables)"
    }
    Write-Host "done - run 'rust-hf-downloader' to start (re-run this one-liner any time to upgrade)"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
