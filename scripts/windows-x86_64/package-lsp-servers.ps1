<#
.SYNOPSIS
    Package LSP Servers Script for Windows x86_64.
.DESCRIPTION
    Creates a .zip per server under .\lsp-packages\, plus sha256sums and a
    manifest-windows-x86_64.json. The manifest schema MUST match PlatformManifest
    in crates/lsp/src/manager/mirror.rs:
      { "platform": ..., "generated": ..., "servers": {
          "<registry-id>": { "filename": ..., "sha256": ...,
                             "size_bytes": N, "version": ... } } }
    Uses built-in Compress-Archive and Get-FileHash - no jq required.
    (Windows clients download .zip; the mirror extractor in downloader.rs
    dispatches on the .zip extension.)

    If PowerShell refuses to run this script with an execution-policy error, run:
      Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
#>

$ErrorActionPreference = 'Continue'

$Platform = "windows-x86_64"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$LspServersDir = Join-Path $ScriptDir "lsp-servers"
$OutputDir = Join-Path $ScriptDir "lsp-packages"
# The registry: each server's license files, and the servers the manifest is
# checked against.
$RegistryJson = if ($env:DEVSCRIPTOR_REGISTRY_JSON) { $env:DEVSCRIPTOR_REGISTRY_JSON } `
                else { Join-Path $ScriptDir "..\..\crates\lsp\src\manager\registry.json" }

function Write-Ok($m)   { Write-Host "[OK] $m" -ForegroundColor Green }
function Write-Err($m)  { Write-Host "ERROR: $m" -ForegroundColor Red }
function Write-Inf($m)  { Write-Host "$m" -ForegroundColor Cyan }
function Write-Warn2($m){ Write-Host "[!] $m" -ForegroundColor Yellow }

# The version of a packaged server: what its download recorded in .version
# (every pinned build, and clangd's release), else what rust-analyzer and gopls
# report, else the day it is packaged.
function Get-ServerVersion([string]$serverId) {
    $dir = Join-Path $LspServersDir $serverId
    $version = ""
    $verFile = Join-Path $dir ".version"
    if (Test-Path $verFile) { $version = (Get-Content $verFile -Raw).Trim() }
    if (-not $version) {
        switch ($serverId) {
            "rust-analyzer" {
                $b = Join-Path $dir "bin\rust-analyzer.exe"
                # `rust-analyzer 0.3.2963-standalone (...)`
                if (Test-Path $b) { $version = ((& $b --version 2>$null) -join " " | Select-String -Pattern '\d+\.\d+\.\d+').Matches.Value | Select-Object -First 1 }
            }
            "gopls" {
                $b = Join-Path $dir "bin\gopls.exe"
                if (Test-Path $b) { $version = ((& $b version 2>$null) -join " " | Select-String -Pattern '\d+\.\d+\.\d+').Matches.Value | Select-Object -First 1 }
            }
        }
    }
    if (-not $version) { $version = (Get-Date -Format "yyyy-MM-dd") }
    return $version
}

# The servers this run packaged, by registry id (each directory's name): the
# checksums and the manifest are of these and only these.
$script:Packaged = [ordered]@{}

# Put the server's license files, as registry.json names them, in its
# LICENSES\: copied from the server's own files where it ships them, else
# fetched from a commit of its publisher's repository and checked against the
# SHA-256 the registry pins, each under its own name or the `name` the registry
# gives it. A server without them is not packaged.
function Add-LicenseFiles([string]$serverId) {
    $dir = Join-Path $LspServersDir $serverId
    $licenses = Join-Path $dir "LICENSES"
    if (Test-Path $licenses) { Remove-Item -Recurse -Force $licenses }
    New-Item -ItemType Directory -Path $licenses | Out-Null
    $files = @($script:RegistryServers.$serverId.license_files | Where-Object { $_ })
    if ($files.Count -eq 0) {
        Write-Err "${serverId}: registry.json names no license file"
        return $false
    }
    foreach ($file in $files) {
        if ($file.path) {
            # A glob where a directory is named by a version.
            $found = Get-ChildItem -Path (Join-Path $dir $file.path) -File -ErrorAction SilentlyContinue | Select-Object -First 1
            if (-not $found) {
                Write-Err "${serverId}: no $($file.path) to take its license from"
                return $false
            }
            $name = if ($file.name) { $file.name } else { $found.Name }
            Copy-Item -Path $found.FullName -Destination (Join-Path $licenses $name)
        } else {
            $name = if ($file.name) { $file.name } else { ($file.url -split '/')[-1] }
            $dest = Join-Path $licenses $name
            $fetched = $true
            try {
                Invoke-WebRequest -Uri $file.url -OutFile $dest -UseBasicParsing -ErrorAction Stop
            } catch {
                $fetched = $false
            }
            if ((-not $fetched) -or ((Get-FileHash -Algorithm SHA256 $dest).Hash.ToLower() -ne $file.sha256)) {
                Remove-Item $dest -ErrorAction SilentlyContinue
                Write-Err "${serverId}: $($file.url) could not be fetched, or is not the file the registry pins"
                return $false
            }
        }
    }
    return $true
}

function New-ServerArchive([string]$serverId) {
    $dir = Join-Path $LspServersDir $serverId
    $version = Get-ServerVersion $serverId
    $archiveName = "$serverId-$version-$Platform.zip"
    $archivePath = Join-Path $OutputDir $archiveName
    Write-Inf "Packaging $serverId (version: $version)..."
    if (-not (Add-LicenseFiles $serverId)) { return $false }
    # Compress-Archive (built into PowerShell 5+). Passing the server directory
    # (not <dir>\*) keeps the top-level "<serverId>/" folder inside the zip, so
    # the layout matches the .tar.gz archives on the other platforms.
    try {
        Compress-Archive -Path $dir -DestinationPath $archivePath -CompressionLevel Optimal -ErrorAction Stop
    } catch {
        Remove-Item $archivePath -ErrorAction SilentlyContinue
        Write-Err "Failed to create archive for ${serverId}: $_"
        return $false
    }
    $mb = "{0:N1} MB" -f ((Get-Item $archivePath).Length / 1MB)
    $script:Packaged[$serverId] = [ordered]@{ filename = $archiveName; version = $version }
    Write-Ok "Created: $archiveName ($mb)"
    return $true
}

function Write-Checksums {
    Write-Inf "Generating checksums..."
    $checksumFile = Join-Path $OutputDir "sha256sums-$Platform.txt"
    $lines = foreach ($entry in $script:Packaged.Values) {
        $h = (Get-FileHash -Algorithm SHA256 (Join-Path $OutputDir $entry.filename)).Hash.ToLower()
        "$h  $($entry.filename)"
    }
    Set-Content -Path $checksumFile -Value $lines -Encoding ASCII
    Write-Ok "Checksums written to: $checksumFile"
}

function New-Manifest {
    Write-Inf "Creating manifest..."
    $manifestFile = Join-Path $OutputDir "manifest-$Platform.json"
    $servers = [ordered]@{}
    foreach ($id in $script:Packaged.Keys) {
        $entry = $script:Packaged[$id]
        $archivePath = Join-Path $OutputDir $entry.filename
        $servers[$id] = [ordered]@{
            filename   = $entry.filename
            sha256     = (Get-FileHash -Algorithm SHA256 $archivePath).Hash.ToLower()
            size_bytes = [int64](Get-Item $archivePath).Length
            version    = $entry.version
        }
    }

    $manifest = [ordered]@{
        platform  = $Platform
        generated = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        servers   = $servers
    }
    # -Depth 5 keeps the nested server objects; ASCII/UTF8 without BOM is safest for the Rust parser.
    $json = $manifest | ConvertTo-Json -Depth 5
    [System.IO.File]::WriteAllText($manifestFile, $json, (New-Object System.Text.UTF8Encoding($false)))
    Write-Ok "Manifest created: $manifestFile"
}

function Test-AgainstRegistry {
    $manifestFile = Join-Path $OutputDir "manifest-$Platform.json"
    $regServers = $script:RegistryServers
    # A server that comes with its toolchain (sourcekit-lsp), or whose builds
    # expire (kotlin-lsp), is never packaged.
    $regIds = $regServers.PSObject.Properties | Where-Object { $_.Value.source.type -ne "system" -and -not $_.Value.builds_expire } | ForEach-Object { $_.Name }
    $man = Get-Content $manifestFile -Raw | ConvertFrom-Json
    $manIds = $man.servers.PSObject.Properties.Name

    $unknown = $manIds | Where-Object { $_ -notin $regIds }
    $absent  = $regIds | Where-Object { $_ -notin $manIds }

    if ($unknown) { Write-Warn2 "Manifest IDs NOT in registry (name the directory after its registry id):"; $unknown | ForEach-Object { Write-Host "    - $_" } }
    if ($absent)  { Write-Warn2 "Registry servers absent from this manifest (Tier-2 fallback at runtime):"; $absent | ForEach-Object { Write-Host "    - $_" } }
    if (-not $unknown -and -not $absent) { Write-Ok "Manifest IDs match registry exactly ($($regIds.Count) servers)" }
}

# ---- Main ----
Write-Inf "LSP Server Packaging Script for $Platform"
Write-Inf "=========================================="

if (-not (Test-Path $RegistryJson)) {
    Write-Err "registry.json not found at $RegistryJson; it names each server's license files"
    Write-Inf "Set DEVSCRIPTOR_REGISTRY_JSON to point at it"
    exit 1
}
$registry = Get-Content $RegistryJson -Raw | ConvertFrom-Json
$script:RegistryServers = if ($registry.PSObject.Properties.Name -contains "servers") { $registry.servers } else { $registry }
if (-not (Test-Path $LspServersDir)) {
    Write-Err "LSP servers directory not found: $LspServersDir"
    Write-Inf "Please run: .\download-lsp-servers.ps1"
    exit 1
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
# An earlier run's archives would be checksummed and uploaded with this run's,
# and its manifest would name servers this run no longer packages.
Remove-Item -Path (Join-Path $OutputDir "*-$Platform.zip"), (Join-Path $OutputDir "sha256sums-$Platform.txt"), (Join-Path $OutputDir "manifest-$Platform.json") -ErrorAction SilentlyContinue

$total = 0; $ok = 0; $fail = 0
# An install a download left unfinished (<id>.new) is no server.
foreach ($d in Get-ChildItem $LspServersDir -Directory | Where-Object { $_.Name -notlike "*.new" }) {
    $total++
    if (New-ServerArchive $d.Name) { $ok++ } else { $fail++ }
}

Write-Host ""
Write-Inf "Packaging Summary"
Write-Inf "================="
Write-Inf "Total servers: $total"
Write-Ok  "Successfully packaged: $ok"
if ($fail -gt 0) { Write-Warn2 "Failed: $fail" }

Write-Host ""
Write-Checksums
New-Manifest
Test-AgainstRegistry

Write-Host ""
Write-Ok "Packaging complete!"
Write-Inf "Archives location: $OutputDir"
Write-Inf "To install from them, see `"Testing packages locally`" in docs/building.md"
