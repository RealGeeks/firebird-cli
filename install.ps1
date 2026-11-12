<#
.SYNOPSIS
  Install the firebird-cli CLI on Windows from its GitHub release archives.

  irm https://raw.githubusercontent.com/realgeeks/firebird-cli/v0.2.0/install.ps1 | iex   # x-release-please-version

  `iex` runs the script with no arguments, so -Version and -InstallDir reach it
  only from the environment, or by turning the download into a scriptblock
  ($url being the address above):

    $env:FIREBIRD_CLI_VERSION = 'v0.3.0'
    irm $url | iex

    & ([scriptblock]::Create((irm $url))) -Version v0.3.0

  Environment / parameters:
    -Version vX.Y.Z        FIREBIRD_CLI_VERSION      a tag; default: the latest release
    -InstallDir DIR        FIREBIRD_CLI_INSTALL_DIR  default: %LOCALAPPDATA%\Programs\firebird-cli
    FIREBIRD_CLI_REPO                                default: realgeeks/firebird-cli
    GH_TOKEN / GITHUB_TOKEN                          optional; avoids GitHub's anonymous
                                                     API rate limit
    FIREBIRD_CLI_SKIP_PROVENANCE=1                   skip the attestation check

  Verifies the sha256 the Release workflow publishes beside each archive and,
  when the GitHub CLI is installed and logged in, the archive's build-provenance
  attestation (as a build of realgeeks/firebird-cli's release workflow, whatever
  FIREBIRD_CLI_REPO says); then adds the install directory to the user PATH.
#>
[CmdletBinding()]
param(
  [string]$Version = $env:FIREBIRD_CLI_VERSION,
  [string]$InstallDir = $env:FIREBIRD_CLI_INSTALL_DIR
)
$ErrorActionPreference = 'Stop'

$repo = if ($env:FIREBIRD_CLI_REPO) { $env:FIREBIRD_CLI_REPO } else { 'realgeeks/firebird-cli' }
$token = if ($env:GH_TOKEN) { $env:GH_TOKEN } elseif ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } else { $null }
$headers = @{ 'Accept' = 'application/vnd.github+json'; 'User-Agent' = 'firebird-cli-install' }
if ($token) { $headers['Authorization'] = "Bearer $token" }

$arch = if ([System.Environment]::Is64BitOperatingSystem) { 'x86_64' } else { throw 'firebird-cli needs 64-bit Windows' }
$target = "$arch-pc-windows-msvc"

if ($Version) {
  if (-not $Version.StartsWith('v')) { $Version = "v$Version" }
  $releaseUrl = "https://api.github.com/repos/$repo/releases/tags/$Version"
} else {
  $releaseUrl = "https://api.github.com/repos/$repo/releases/latest"
}
try {
  $release = Invoke-RestMethod -Uri $releaseUrl -Headers $headers
} catch {
  throw "cannot read the release ($releaseUrl). If GitHub is rate limiting you, set GH_TOKEN. $_"
}
$tag = $release.tag_name
$name = "firebird-cli-$tag-$target"
$archive = "$name.zip"

function Get-Asset([string]$assetName, [string]$dest) {
  if ($token) {
    $asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
    if (-not $asset) { throw "release $tag has no asset $assetName" }
    $h = $headers.Clone(); $h['Accept'] = 'application/octet-stream'
    Invoke-WebRequest -Uri $asset.url -Headers $h -OutFile $dest
  } else {
    Invoke-WebRequest -Uri "https://github.com/$repo/releases/download/$tag/$assetName" -Headers @{ 'User-Agent' = 'firebird-cli-install' } -OutFile $dest
  }
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("firebird-cli-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "Downloading firebird-cli $tag for $target..."
  Get-Asset $archive (Join-Path $tmp $archive)
  Get-Asset "$archive.sha256" (Join-Path $tmp "$archive.sha256")

  $expected = ((Get-Content (Join-Path $tmp "$archive.sha256") -Raw) -split '\s+')[0].ToLower()
  $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw "checksum mismatch for $archive (expected $expected, got $actual)" }

  # Build provenance: the sha256 comes from the same release as the archive,
  # the attestation from Sigstore. Always against the official repository.
  $official = 'RealGeeks/firebird-cli'
  $signer = "https://github.com/$official/.github/workflows/release.yml@refs/heads/main"
  $verifyCmd = "gh attestation verify $archive --repo $official --cert-identity $signer"
  $gh = Get-Command gh -ErrorAction SilentlyContinue
  $ghReady = $false
  if ($gh -and -not $env:FIREBIRD_CLI_SKIP_PROVENANCE) {
    # Windows PowerShell turns a native command's stderr into an error under 'Stop'
    $ErrorActionPreference = 'Continue'
    $ghReady = [bool]$token
    if (-not $ghReady) {
      & $gh.Source auth status *> $null
      $ghReady = ($LASTEXITCODE -eq 0)
    }
    if ($ghReady) {
      & $gh.Source attestation verify (Join-Path $tmp $archive) --repo $official --cert-identity $signer *> $null
      $verified = ($LASTEXITCODE -eq 0)
    }
    $ErrorActionPreference = 'Stop'
  }
  if ($env:FIREBIRD_CLI_SKIP_PROVENANCE) {
    Write-Warning 'FIREBIRD_CLI_SKIP_PROVENANCE is set; build provenance not checked'
  } elseif ($ghReady) {
    if (-not $verified) { throw "$archive failed its build-provenance check: it was not built by $official's release workflow, or gh could not reach GitHub. Nothing was installed. To see why: $verifyCmd (FIREBIRD_CLI_SKIP_PROVENANCE=1 skips the check)" }
    Write-Host "Build provenance verified ($official release workflow)."
  } else {
    Write-Host 'Build provenance not verified (needs the GitHub CLI, logged in). To check it yourself:'
    Write-Host "  $verifyCmd"
  }

  Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force
  $exe = Join-Path $tmp "$name\firebird-cli.exe"
  if (-not (Test-Path $exe)) { throw "$archive did not contain $name\firebird-cli.exe" }

  if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\firebird-cli' }
  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  Copy-Item $exe (Join-Path $InstallDir 'firebird-cli.exe') -Force

  $installed = & (Join-Path $InstallDir 'firebird-cli.exe') --version
  Write-Host "Installed $InstallDir\firebird-cli.exe ($installed)"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($userPath -split ';') -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable('Path', "$InstallDir;$userPath", 'User')
    $env:Path = "$InstallDir;$env:Path"
    Write-Host "Added $InstallDir to your PATH (open a new terminal to pick it up)."
  }
  Write-Host 'Next: firebird-cli auth login --site www.yoursite.com --api-key-stdin   (then: firebird-cli guide)'
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
