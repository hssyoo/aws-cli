# AWS CLI v2 install script for Windows (PowerShell 5.1+).
#
# Defaults to a user-local install under %LOCALAPPDATA%. Use -System for the
# traditional Program Files layout (requires admin). See -Help for details.
#
# This is the customer-facing entry point intended to be run as:
#   irm 'https://awscli.amazonaws.com/v2/install.ps1' | iex

[CmdletBinding()]
param(
    [string] $Version,
    [switch] $System,
    [switch] $Quiet,
    [string] $DevInstallScript,  # TEMPORARY: bypasses signature checks if set
    [switch] $Help
)

$ErrorActionPreference = 'Stop'

# PowerShell 5.1 doesn't default to TLS 1.2 — required for awscli.amazonaws.com
# and the dev CloudFront origin. Newer S3/CloudFront endpoints reject TLS 1.0.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# ----- Constants -------------------------------------------------------------

$ScriptVersion = '0.0.0-dev'

# TEMPORARY: dev CloudFront origin used for preview builds. Will be pointed
# back at https://awscli.amazonaws.com before launch.
$DownloadBaseUrl    = 'https://d2j4ws18f0zfbf.cloudfront.net'
$LatestVersionUrl   = "$DownloadBaseUrl/v2/version.txt"

# distribution_source identifier — propagates to the artifact download
# query string, install.json, and (transitively) the user-agent. Override
# via AWS_CLI_DISTRIBUTION_SOURCE_OVERRIDE; `aws update` sets it to "update-exe".
$DistributionSource = if ($env:AWS_CLI_DISTRIBUTION_SOURCE_OVERRIDE) {
    $env:AWS_CLI_DISTRIBUTION_SOURCE_OVERRIDE
} else {
    'script-exe'
}
$DownloadQuery = "?src=$DistributionSource"

# msiexec exit codes that indicate success.
# 0    = success, 1641 = success but reboot initiated, 3010 = reboot required.
$MsiexecSuccessCodes = @(0, 1641, 3010)

# ----- Logging ---------------------------------------------------------------

# Stdout is silenced by -Quiet. Stderr is never silenced.
function Write-Info {
    param([string] $Message)
    if (-not $Quiet) { Write-Host $Message }
}

function Write-Warn {
    param([string] $Message)
    [Console]::Error.WriteLine("aws-cli installer: warning: $Message")
}

function Throw-Error {
    param([int] $Code, [string] $Message)
    [Console]::Error.WriteLine("aws-cli installer: error: $Message")
    exit $Code
}

# ----- Help ------------------------------------------------------------------

function Show-Help {
    @"
Usage: install.ps1 [-Version <X.Y.Z>] [-System] [-Quiet] [-Help]

Install or update the AWS CLI v2.

Parameters:
  -Version <X.Y.Z>  Install a specific fully-qualified version. Otherwise
                    installs the latest release.
  -System           Install system-wide (requires admin). Otherwise installs
                    under %LOCALAPPDATA%\Programs\Amazon\AWSCLIV2\.
  -Quiet            Suppress non-essential output. Errors and warnings are
                    still printed to stderr.
  -Help             Show this help and exit.
"@ | Write-Host
}

# ----- Argument validation ---------------------------------------------------

function Test-Semver {
    param([string] $Value)
    return ($Value -match '^\d+\.\d+\.\d+$')
}

function Validate-Args {
    if ($Help) { Show-Help; exit 0 }
    if ($Version -and -not (Test-Semver $Version)) {
        Throw-Error 2 "-Version must be fully-qualified semver (e.g. 2.27.41), got: $Version"
    }
}

# ----- Platform / privilege --------------------------------------------------

function Test-Windows {
    if ($PSVersionTable.PSVersion.Major -ge 6) {
        return $IsWindows
    }
    # PowerShell 5.1 has no $IsWindows but only runs on Windows.
    return $true
}

function Assert-Platform {
    if (-not (Test-Windows)) {
        Throw-Error 1 'unsupported OS. install.ps1 supports only Windows; use install.sh on Linux/macOS.'
    }
}

function Test-IsAdmin {
    $identity  = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-SystemPrivilege {
    if ($System -and -not (Test-IsAdmin)) {
        Throw-Error 1 '-System requires admin privileges. Re-run from an elevated PowerShell, or omit -System for a user-local install.'
    }
}

# ----- Path resolution -------------------------------------------------------

$Script:InstallDir = $null

function Resolve-InstallPath {
    if ($System) {
        $Script:InstallDir = Join-Path $env:ProgramFiles 'Amazon\AWSCLIV2'
    } else {
        if (-not $env:LOCALAPPDATA) {
            Throw-Error 2 'LOCALAPPDATA is not set; cannot resolve user-local install path.'
        }
        $Script:InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Amazon\AWSCLIV2'
    }
}

# Best-effort detection of an existing install in the *other* layout (system
# vs user-local) — warns the user that two installs will coexist and PATH
# resolution decides which `aws` runs.
function Warn-ExistingInstall {
    $otherRoot = if ($System) {
        Join-Path $env:LOCALAPPDATA 'Programs\Amazon\AWSCLIV2'
    } else {
        Join-Path $env:ProgramFiles 'Amazon\AWSCLIV2'
    }
    $otherLabel = if ($System) { 'user-local' } else { 'system-wide' }

    if (Test-Path (Join-Path $otherRoot 'aws.exe')) {
        Write-Warn "existing $otherLabel AWS CLI install found at $otherRoot. Both installs will coexist; PATH precedence will determine which 'aws' runs."
    }
}

# ----- Temp directory + cleanup ----------------------------------------------

$Script:TempDir = $null

function New-TempDir {
    $Script:TempDir = Join-Path $env:TEMP "aws-cli-install-$([guid]::NewGuid().Guid)"
    New-Item -ItemType Directory -Path $Script:TempDir -Force | Out-Null
}

function Remove-TempDir {
    if ($Script:TempDir -and (Test-Path $Script:TempDir)) {
        try {
            Remove-Item -Recurse -Force $Script:TempDir -ErrorAction Stop
        } catch {
            Write-Warn "could not remove temp directory: $Script:TempDir"
        }
    }
}

# ----- Download --------------------------------------------------------------

function Get-InstallerFilename {
    if ($Script:TargetVersion) {
        return "AWSCLIV2-$($Script:TargetVersion).msi"
    }
    return 'AWSCLIV2.msi'
}

function Get-InstallerUrl {
    return "$DownloadBaseUrl/$(Get-InstallerFilename)$DownloadQuery"
}

# Invoke-WebRequest in PS 5.1 parses HTML responses by default and is slow
# for binary downloads; -UseBasicParsing is the standard workaround.
function Download-File {
    param([string] $Url, [string] $Out)
    $params = @{
        Uri              = $Url
        OutFile          = $Out
        UseBasicParsing  = $true
        TimeoutSec       = 60
    }
    Invoke-WebRequest @params
}

# E6 semantics: one initial attempt, one retry on failure, then propagate.
function Download-WithRetry {
    param([string] $Url, [string] $Out, [string] $Label)
    Write-Info "Downloading $Label from $Url"
    try {
        Download-File -Url $Url -Out $Out
        return
    } catch {
        Write-Warn "download of $Label failed; retrying once"
    }
    try {
        Download-File -Url $Url -Out $Out
    } catch {
        Throw-Error 1 "failed to download $Label from $Url after one retry: $($_.Exception.Message)"
    }
}

$Script:InstallerPath = $null

function Download-Installer {
    $Script:InstallerPath = Join-Path $Script:TempDir 'AWSCLIV2.msi'
    Download-WithRetry -Url (Get-InstallerUrl) -Out $Script:InstallerPath -Label 'AWS CLI installer'
}

# ----- Integrity verification ------------------------------------------------

function Verify-Installer {
    # TEMPORARY: signature verification disabled while pointing at the dev
    # CloudFront origin. Preview-build MSIs there are not Authenticode-signed
    # by Amazon. Re-enable by replacing this body with a Get-AuthenticodeSignature
    # check pinned to the production signer subject before launch.
    Write-Warn 'signature verification disabled (dev preview build)'
}

# ----- Pre/post version capture ---------------------------------------------

function Read-InstalledVersion {
    param([string] $AwsExe)
    if (-not (Test-Path $AwsExe)) { return $null }
    try {
        $out = & $AwsExe --version 2>$null
    } catch {
        return $null
    }
    if ($out -match '^aws-cli/(\d+\.\d+\.\d+)') {
        return $Matches[1]
    }
    return $null
}

function Get-CandidateAwsBinary {
    return Join-Path $Script:InstallDir 'aws.exe'
}

$Script:PreInstallVersion  = $null
$Script:TargetVersion      = $null
$Script:PostInstallVersion = $null

function Capture-PreInstallVersion {
    $Script:PreInstallVersion = Read-InstalledVersion (Get-CandidateAwsBinary)
}

# Best-effort GET of the version manifest. Returns $null on any failure.
function Fetch-LatestVersion {
    try {
        $resp = Invoke-WebRequest -Uri $LatestVersionUrl -UseBasicParsing -TimeoutSec 10
        $text = ([string] $resp.Content).Trim()
        if (Test-Semver $text) { return $text }
    } catch {}
    return $null
}

function Resolve-TargetAndAnnounce {
    if ($Version) {
        $Script:TargetVersion = $Version
    } else {
        $Script:TargetVersion = Fetch-LatestVersion
    }

    if ($Script:TargetVersion -and $Script:PreInstallVersion -and `
        $Script:TargetVersion -eq $Script:PreInstallVersion) {
        Write-Info "AWS CLI $($Script:TargetVersion) is already installed at $Script:InstallDir; nothing to do."
        exit 0
    }

    if ($Script:TargetVersion) {
        if ($Script:PreInstallVersion) {
            Write-Info "Installing AWS CLI $Script:PreInstallVersion → $Script:TargetVersion"
        } else {
            Write-Info "Installing AWS CLI $Script:TargetVersion"
        }
    }
}

# ----- Install ---------------------------------------------------------------

function Run-Installer {
    # The bundled MSI also writes install.json; tell it to skip so our
    # post-install write is the single canonical writer.
    $env:AWS_CLI_SKIP_INSTALL_JSON = '1'

    $args = @(
        '/i', $Script:InstallerPath,
        '/qn',           # silent, no UI
        '/norestart'     # don't auto-reboot for code 3010
    )

    if (-not $System) {
        # Per-user install mode: TRANSFORMS=":PerUser" applies the embedded
        # transform that switches MSI install context to per-user. The other
        # three properties are required alongside it. We don't pass INSTALLDIR
        # — the transform owns destination placement.
        $args += @(
            'TRANSFORMS=:PerUser',
            'MSINEWINSTANCE=1',
            'MSIINSTALLPERUSER=1',
            'ALLUSERS=2'
        )
    }

    Write-Info 'Running MSI installer...'
    $proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList $args `
                          -Wait -PassThru -NoNewWindow
    if ($MsiexecSuccessCodes -notcontains $proc.ExitCode) {
        Throw-Error 8 "msiexec failed with exit code $($proc.ExitCode)"
    }
}

# ----- Post-install ---------------------------------------------------------

function Verify-InstallRuns {
    $awsExe = Get-CandidateAwsBinary
    $Script:PostInstallVersion = Read-InstalledVersion $awsExe
    if (-not $Script:PostInstallVersion) {
        Throw-Error 9 "post-install check failed: '$awsExe --version' did not run successfully."
    }
}

# Write install.json next to the build-time metadata.json. Values are
# already constrained (paths come from %ProgramFiles%/%LOCALAPPDATA%, version
# is validated semver, the rest are booleans), so we emit JSON via here-string.
function Write-InstallJson {
    $installJson = Join-Path $Script:InstallDir 'awscli\data\install.json'
    $parent = Split-Path $installJson -Parent
    if (-not (Test-Path $parent)) { return }

    $systemBool = if ($System) { 'true' } else { 'false' }
    $quietBool  = if ($Quiet)  { 'true' } else { 'false' }

    @"
{
  "distribution_source": "$DistributionSource",
  "install_dir": "$($Script:InstallDir -replace '\\','\\\\')",
  "script_install": {
    "system": $systemBool,
    "version_resolved": "$Script:PostInstallVersion",
    "quiet": $quietBool,
    "script_version": "$ScriptVersion"
  }
}
"@ | Set-Content -Path $installJson -Encoding UTF8
}

# Trust the MSI's PATH handling — we don't post-process or verify PATH from
# the script. The MSI is the canonical owner of PATH on Windows installs.

function Invoke-PostInstallChecks {
    Verify-InstallRuns
    Write-InstallJson
    Warn-ExistingInstall
    Write-Info "AWS CLI $Script:PostInstallVersion installed to $Script:InstallDir"
}

# ----- Main ------------------------------------------------------------------

try {
    Validate-Args
    Assert-Platform
    Assert-SystemPrivilege
    Resolve-InstallPath

    New-TempDir
    Capture-PreInstallVersion
    Resolve-TargetAndAnnounce
    Download-Installer
    Verify-Installer
    Run-Installer
    Invoke-PostInstallChecks
} finally {
    Remove-TempDir
}
