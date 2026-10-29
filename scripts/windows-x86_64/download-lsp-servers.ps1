<#
.SYNOPSIS
    Download script for 20 LSP servers - Windows x86_64.
.DESCRIPTION
    Installs each LSP server into .\lsp-servers\<id>\ using the appropriate tool
    (npm / go / gem / dotnet / coursier / GitHub release / system copy). Run
    install-dependencies.ps1 first, then open a fresh shell so tools are on PATH.

    If PowerShell refuses to run this script with an execution-policy error, run:
      Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
#>

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'   # speeds up Invoke-WebRequest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$LspDir = Join-Path $ScriptDir "lsp-servers"

# The token GitHub's API is asked with, where one is given (the LSP mirror
# workflow gives its own): without one, the API answers 60 requests an hour
# from an address, which CI runners share. It is taken out of the
# environment, so that no installer this script runs (npm, gem, go, dotnet)
# sees it.
$script:GitHubApiToken = $env:GITHUB_TOKEN
Remove-Item Env:GITHUB_TOKEN -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $LspDir | Out-Null

Write-Host "==============================================="
Write-Host "Downloading 20 LSP Servers"
Write-Host "Platform: Windows x86_64"
Write-Host "==============================================="
Write-Host "Target directory: $LspDir"
Write-Host ""

$script:Downloaded = @{}
$script:Failed = @{}

function Test-Cmd([string]$n) { $null -ne (Get-Command $n -ErrorAction SilentlyContinue) }
function Mark-Downloaded($n, $how) { $script:Downloaded[$n] = $how; Write-Host "[$n] OK ($how)" }
function Mark-Failed($n, $why) { $script:Failed[$n] = $why; Write-Host "[$n] FAILED - $why" }
function Already-Present($n) { $script:Downloaded[$n] = "already present"; Write-Host "[$n] Already present, skipping" }

# --- npm (typescript, volar, svelte, html, graphql, pyright, sql, bash, solidity) ---
# Install $package at $version, the build registry.json pins (direct installs
# fetch the same), into lsp-servers\$name\, with the companions registry.json
# lists for it: npm packages it needs but does not depend on
# (typescript-language-server and volar refuse to start without a `typescript`
# to point at; sql-language-server reaches into
# vscode-languageserver-protocol's internals, which newer versions hide behind
# an `exports` map). .version records the build, and an install of another
# version is replaced once the new one is in (see Install-Swapped).
function Download-Npm($name, $package, $version, [string[]]$companions = @()) {
    $target = Join-Path $LspDir $name
    if (Test-PinnedVersion (Join-Path $target ".version") $version) { Already-Present $name; return }
    Write-Host "[$name] Installing via npm: $package@$version $($companions -join ' ')"
    $staging = New-Staging $target
    Push-Location $staging
    # npm ships as npm.cmd on Windows
    & npm.cmd install --omit=dev "$package@$version" @companions 2>&1 | Select-Object -Last 5 | Out-Host
    $installed = $LASTEXITCODE -eq 0
    Pop-Location
    if ($installed) {
        Set-Content -Path (Join-Path $staging ".version") -Value $version -Encoding ASCII
        Install-Swapped $target
        Mark-Downloaded $name "npm:$package@$version"
    } else {
        Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
        Mark-Failed $name "npm failed"
    }
}

# A server is installed beside the one it replaces, in "<dir>.new", so that an
# install that fails (no npm, no dotnet, no network) leaves the old one as it
# was: New-Staging makes that directory, Install-Swapped puts it in place.
function New-Staging([string]$Target) {
    $staging = "$Target.new"
    if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    return $staging
}

function Install-Swapped([string]$Target) {
    if (Test-Path $Target) { Remove-Item -Recurse -Force $Target }
    Move-Item "$Target.new" $Target
}

# --- go install (gopls) ---
function Download-Go($name, $package) {
    $target = Join-Path $LspDir $name
    $bin = Join-Path $target "bin"
    Write-Host "[$name] Installing via go: $package"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    $goErr = [System.IO.Path]::GetTempFileName()
    # Clear GOOS/GOARCH so GOBIN install is not treated as cross-compilation
    # when the caller has those variables set.
    $env:GOBIN = $bin
    $env:GOOS = ""
    $env:GOARCH = ""
    & go install "$package@latest" 2>$goErr | Out-Null
    $ok = $LASTEXITCODE -eq 0
    $exe = Join-Path $bin "$name.exe"
    if ($ok -and (Test-Path $exe)) {
        Mark-Downloaded $name "go:$package"
    } else {
        Mark-Failed $name "go install failed"
        if (Test-Path $goErr) {
            Write-Host "go install output:"
            Get-Content $goErr -Tail 20 | ForEach-Object { Write-Host "  $_" }
        }
    }
    Remove-Item $goErr -ErrorAction SilentlyContinue
}

function Find-Msys2() {
    $candidates = [System.Collections.Generic.List[string]]::new()
    if (Test-Cmd ruby) {
        $rubyExe = (Get-Command ruby).Source
        $rubyRoot = Split-Path (Split-Path $rubyExe) -Parent
        $candidates.Add((Join-Path $rubyRoot "msys64"))
    }
    $candidates.Add("C:\msys64")
    $candidates.Add("$env:ProgramFiles\msys64")
    $candidates.Add("$env:LOCALAPPDATA\Programs\msys64")
    foreach ($root in $candidates) {
        if (Test-Path (Join-Path $root "usr\bin\bash.exe")) { return $root }
    }
    return $null
}

# Try to make RubyInstaller aware of MSYS2 if it isn't already.
function Register-Msys2WithRidk {
    if (-not (Test-Cmd ridk)) { return }
    $info = & ridk version 2>&1 | Out-String
    if ($info -match "msys2") { return }

    $msys2 = Find-Msys2
    if (-not $msys2) { return }

    # Add MSYS2 to PATH; ridk's detection uses known paths, but this also lets
    # ridk exec find the toolchain if detection is only PATH-based.
    foreach ($envName in @("mingw64", "ucrt64")) {
        $toolBin = Join-Path $msys2 "$envName\bin"
        if ((Test-Path $toolBin) -and ($env:Path -notlike "*$toolBin*")) {
            $env:Path = "$toolBin;$env:Path"
        }
    }
    $usrBin = Join-Path $msys2 "usr\bin"
    if ($env:Path -notlike "*$usrBin*") {
        $env:Path = "$usrBin;$env:Path"
    }

    Write-Host "  [ruby-lsp] Running ridk install 3 to register MSYS2/MINGW toolchain..."
    & ridk install 3
}

# Whether the gem bundle's native extensions, under
# extensions\<platform>\<api>\, are this machine's Ruby's, as Devscriptor
# checks before it runs one: another Ruby misses the gems they belong to. A
# bundle with no native extensions runs on any.
function Test-BuiltForThisRuby([string]$Bundle) {
    $extensions = Join-Path $Bundle "extensions"
    if (-not (Test-Path $extensions)) { return $true }
    if (-not (Test-Cmd ruby)) { return $false }
    $api = (& ruby -e "print Gem.extension_api_version" 2>$null)
    if (-not $api) { return $false }
    return [bool](Get-ChildItem $extensions -Directory | Get-ChildItem -Directory | Where-Object { $_.Name -eq $api } | Select-Object -First 1)
}

# --- gem install (ruby-lsp) ---
# Installed beside an earlier install, which it replaces once it is in (see
# Install-Swapped).
function Download-Gem($name, $package, $version) {
    $final = Join-Path $LspDir $name
    $target = New-Staging $final
    $bin = Join-Path $target "bin"
    Write-Host "[$name] Installing via gem: $package"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null

    # Make sure MSYS2 is registered with RubyInstaller before invoking gem.
    Register-Msys2WithRidk

    $gemErr = [System.IO.Path]::GetTempFileName()

    # ridk exec injects the MSYS2/MINGW environment that native gem builds need.
    $pin = @()
    if ($version) { $pin = @("--version", $version) }
    if (Test-Cmd ridk) {
        & ridk exec gem install --install-dir "$target" --bindir "$bin" --no-document @pin $package 2>$gemErr | Out-Null
    } else {
        & gem install --install-dir "$target" --bindir "$bin" --no-document @pin $package 2>$gemErr | Out-Null
    }
    if ($LASTEXITCODE -eq 0) {
        $exe = Join-Path $bin "$name.bat"
        if (-not (Test-Path $exe)) {
            $found = Get-ChildItem $target -Recurse -Filter "$name.bat" | Select-Object -First 1
            if ($found) { Copy-Item $found.FullName $bin -Force }
        }
        if (Test-Path $exe) {
            Install-Swapped $final
            Mark-Downloaded $name "gem:$package"
        } else {
            Remove-Item -Recurse -Force $target -ErrorAction SilentlyContinue
            Mark-Failed $name "gem binary not found"
        }
    } else {
        Remove-Item -Recurse -Force $target -ErrorAction SilentlyContinue
        Mark-Failed $name "gem failed"
        if (Test-Path $gemErr) {
            Write-Host "gem install output:"
            Get-Content $gemErr -Tail 20 | ForEach-Object { Write-Host "  $_" }
        }
    }
    Remove-Item $gemErr -ErrorAction SilentlyContinue
}

# --- dotnet tool (csharp-ls, fsautocomplete) ---
# Install the .NET tool at $version, the build registry.json pins, into
# lsp-servers\$name\bin\. .version records the build, and an install of another
# version is replaced once the new one is in. The dotnet CLI's first-run files
# go to a directory of their own, not into the package.
function Download-Dotnet($name, $package, $version) {
    $target = Join-Path $LspDir $name
    if (Test-PinnedVersion (Join-Path $target ".version") $version) { Already-Present $name; return }
    Write-Host "[$name] Installing via dotnet: $package $version"
    $staging = New-Staging $target
    $cliHome = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $cliHome | Out-Null
    $previousCliHome = $env:DOTNET_CLI_HOME
    $env:DOTNET_CLI_HOME = $cliHome
    $toolArgs = @("tool", "install", "--tool-path", (Join-Path $staging "bin"), $package, "--version", $version)
    & dotnet @toolArgs 2>&1 | Select-Object -Last 5 | Out-Host
    $installed = $LASTEXITCODE -eq 0
    $env:DOTNET_CLI_HOME = $previousCliHome
    Remove-Item -Recurse -Force $cliHome -ErrorAction SilentlyContinue
    if ($installed) {
        Set-Content -Path (Join-Path $staging ".version") -Value $version -Encoding ASCII
        Install-Swapped $target
        Mark-Downloaded $name "dotnet:$package@$version"
    } else {
        Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
        Mark-Failed $name "dotnet failed"
    }
}

# Download $Url to $Dest and check it against the SHA-256 its publisher puts
# beside it ($Url.sha256). A download whose checksum is missing or wrong is
# deleted, and $false returned. The sidecar is read from a file as text: for an
# application/octet-stream response, `.Content` is a Byte[] (see the Dart SDK).
function Get-Verified([string]$Url, [string]$Dest) {
    $expected = $null
    try {
        Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing
        $sumPath = "$Dest.sha256"
        Invoke-WebRequest -Uri "$Url.sha256" -OutFile $sumPath -UseBasicParsing
        $expected = ((Get-Content $sumPath -Raw) -split '\s+', 2)[0].Trim().ToLower()
        Remove-Item $sumPath -ErrorAction SilentlyContinue
    } catch {
        Write-Host "  could not fetch $Url or its checksum: $_"
        Remove-Item $Dest -ErrorAction SilentlyContinue
        return $false
    }
    $actual = (Get-FileHash -Path $Dest -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or ($expected -ne $actual)) {
        Write-Host "  SHA-256 mismatch for $Url (expected $expected, got $actual)"
        Remove-Item $Dest -ErrorAction SilentlyContinue
        return $false
    }
    return $true
}

# Download the asset of $Repo's latest GitHub release whose name matches
# $Pattern to $Dest, checked against the SHA-256 GitHub publishes for it, and
# return the asset's name. A release that cannot be read, no matching asset, no
# published checksum or a download that does not match it returns $null, and
# leaves nothing at $Dest.
function Get-ReleaseAsset([string]$Repo, [string]$Pattern, [string]$Dest) {
    try {
        $headers = @{ "User-Agent" = "devscriptor" }
        if ($script:GitHubApiToken) { $headers["Authorization"] = "Bearer $script:GitHubApiToken" }
        $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers $headers -UseBasicParsing
        $asset = $rel.assets | Where-Object { $_.name -match $Pattern } | Select-Object -First 1
        if (-not $asset) {
            Write-Host "  no asset of the latest $Repo release matches $Pattern"
            return $null
        }
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $Dest -UseBasicParsing
    } catch {
        Write-Host "  could not fetch the latest $Repo release: $_"
        Remove-Item $Dest -ErrorAction SilentlyContinue
        return $null
    }
    $expected = if ($asset.digest) { ($asset.digest -replace '^sha256:', '').ToLower() } else { $null }
    $actual = (Get-FileHash -Path $Dest -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or ($expected -ne $actual)) {
        Write-Host "  SHA-256 mismatch for $($asset.name) (expected $expected, got $actual)"
        Remove-Item $Dest -ErrorAction SilentlyContinue
        return $null
    }
    return $asset.name
}

# Whether the file at $Path holds $Version (a pinned build's version record).
function Test-PinnedVersion([string]$Path, [string]$Version) {
    if (-not (Test-Path $Path)) { return $false }
    return (Get-Content $Path -Raw).Trim() -eq $Version
}

# Whether the file at $Path, binary or not, holds the ASCII text $Text: read
# a block at a time, each with the end of the last, which the text may
# straddle.
function Test-FileHolds([string]$Path, [string]$Text) {
    if (-not (Test-Path $Path)) { return $false }
    $latin1 = [System.Text.Encoding]::GetEncoding(28591)
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $block = New-Object byte[] 1048576
        $carried = ""
        while (($read = $stream.Read($block, 0, $block.Length)) -gt 0) {
            $chunk = $carried + $latin1.GetString($block, 0, $read)
            if ($chunk.Contains($Text)) { return $true }
            $keep = [Math]::Min($Text.Length - 1, $chunk.Length)
            $carried = $chunk.Substring($chunk.Length - $keep)
        }
        return $false
    } finally {
        $stream.Dispose()
    }
}

function Find-Coursier() {
    if (Test-Cmd cs) { return (Get-Command cs).Source }
    if (Test-Cmd coursier) { return (Get-Command coursier).Source }
    $candidates = @(
        Join-Path $env:LOCALAPPDATA "Coursier\bin\cs.exe"
        "$env:ProgramFiles\Coursier\cs.exe"
        Join-Path $env:USERPROFILE ".cargo\bin\cs.exe"
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return $c } }
    return $null
}

Write-Host "`n=== Tier 1: Core Languages (4 servers) ==="

# 1. Rust - rust-analyzer
#
# The binary must come from the upstream release, not from this machine:
# `.cargo\bin\rust-analyzer.exe` is a rustup *proxy* that dies elsewhere with
# "rustup could not choose a version of rust-analyzer to run", and the
# toolchain's own binary links against librustc_driver from inside that
# toolchain. The published `*-standalone` asset has neither problem.
if (Test-Path (Join-Path $LspDir "rust-analyzer\bin\rust-analyzer.exe")) {
    Already-Present "rust-analyzer"
} else {
    $raAsset = "rust-analyzer-x86_64-pc-windows-msvc.zip"
    Write-Host "[rust-analyzer] Downloading the latest release's $raAsset"
    $raTmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $raTmp | Out-Null
    $raZip = Join-Path $raTmp "ra.zip"
    if (-not (Get-ReleaseAsset "rust-lang/rust-analyzer" "^$([regex]::Escape($raAsset))$" $raZip)) {
        Mark-Failed "rust-analyzer" "could not fetch a verified $raAsset"
    } else {
        try {
            Expand-Archive -Path $raZip -DestinationPath $raTmp -Force
            $raExe = Get-ChildItem -Path $raTmp -Filter "rust-analyzer*.exe" -Recurse | Select-Object -First 1
            if (-not $raExe) { throw "rust-analyzer.exe not in archive" }
            $raBin = Join-Path $LspDir "rust-analyzer\bin"
            New-Item -ItemType Directory -Force -Path $raBin | Out-Null
            Copy-Item $raExe.FullName (Join-Path $raBin "rust-analyzer.exe") -Force
            # A rustup proxy would fail here; the standalone build does not.
            $ver = & (Join-Path $raBin "rust-analyzer.exe") --version 2>$null
            if ($LASTEXITCODE -eq 0) { Mark-Downloaded "rust-analyzer" "github:rust-lang/rust-analyzer" }
            else { Mark-Failed "rust-analyzer" "downloaded binary does not run" }
        } catch {
            Mark-Failed "rust-analyzer" "unpack failed: $_"
        }
    }
    Remove-Item -Recurse -Force $raTmp -ErrorAction SilentlyContinue
}

# 2. C/C++ - clangd
#
# Like rust-analyzer, clangd comes from its upstream release, not from this
# machine: the `clangd.exe` on PATH may be another toolchain's, which needs
# that toolchain's DLLs (the Swift toolchain ships one). The release's is one
# static binary, with the builtin headers it needs.
#
# clangd finds those headers in a "resource directory" whose absolute path is
# baked into the binary at build time, so the release's
# lib\clang\<major>\include travels as resource-dir\include, and the registry
# launches clangd with `--resource-dir={server_dir}/resource-dir`: clangd
# ignores headers that are only shipped beside it. `.version` records the
# release, so a copy an earlier download took from PATH is replaced.
$clangdDir = Join-Path $LspDir "clangd"
if ((Test-Path (Join-Path $clangdDir "bin\clangd.exe")) -and
    (Test-Path (Join-Path $clangdDir "resource-dir\include")) -and
    (Test-Path (Join-Path $clangdDir ".version"))) {
    Already-Present "clangd"
} else {
    Write-Host "[clangd] Downloading the latest clangd release"
    $clangdTmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $clangdTmp | Out-Null
    $clangdZip = Join-Path $clangdTmp "clangd.zip"
    $clangdAsset = Get-ReleaseAsset "clangd/clangd" '^clangd-windows-[0-9.]+\.zip$' $clangdZip
    $clangdInclude = $null
    if ($clangdAsset) {
        $clangdVersion = $clangdAsset -replace '^clangd-windows-(.+)\.zip$', '$1'
        $clangdRoot = Join-Path $clangdTmp "clangd_$clangdVersion"
        try {
            Expand-Archive -Path $clangdZip -DestinationPath $clangdTmp -Force
            $clangdInclude = Get-ChildItem -Path (Join-Path $clangdRoot "lib\clang") -Directory -ErrorAction SilentlyContinue |
                ForEach-Object { Join-Path $_.FullName "include" } |
                Where-Object { Test-Path $_ } | Select-Object -First 1
        } catch {
            Write-Host "  could not unpack ${clangdAsset}: $_"
        }
    }
    if ($clangdInclude -and (Test-Path (Join-Path $clangdRoot "bin\clangd.exe"))) {
        if (Test-Path $clangdDir) { Remove-Item -Recurse -Force $clangdDir }
        New-Item -ItemType Directory -Force -Path (Join-Path $clangdDir "bin") | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $clangdDir "resource-dir") | Out-Null
        Copy-Item (Join-Path $clangdRoot "bin\clangd.exe") (Join-Path $clangdDir "bin\clangd.exe") -Force
        Copy-Item $clangdInclude (Join-Path $clangdDir "resource-dir\include") -Recurse -Force
        Set-Content -Path (Join-Path $clangdDir ".version") -Value $clangdVersion -Encoding ASCII
        Mark-Downloaded "clangd" "github:clangd/clangd $clangdVersion"
    } else {
        Mark-Failed "clangd" "could not fetch a verified clangd release with bin\clangd.exe and lib\clang\<major>\include"
    }
    Remove-Item -Recurse -Force $clangdTmp -ErrorAction SilentlyContinue
}

# 3. Go - gopls
if (Test-Path (Join-Path $LspDir "gopls\bin\gopls.exe")) { Already-Present "gopls" }
elseif (Test-Cmd go) { Download-Go "gopls" "golang.org/x/tools/gopls" }
else { Mark-Failed "gopls" "go not available" }

# 4. TypeScript/JavaScript - typescript-language-server
Download-Npm "typescript-language-server" "typescript-language-server" "5.3.0" @("typescript@5.9.3")

Write-Host "`n=== Tier 2: Web Frontend (4 servers) ==="
Download-Npm "volar" "@vue/language-server" "2.2.12" @("typescript@5.9.3")
Download-Npm "svelte-language-server" "svelte-language-server" "0.18.3"
Download-Npm "html-languageserver" "vscode-langservers-extracted" "4.10.0"
Download-Npm "graphql-language-service" "graphql-language-service-cli" "3.5.0"

Write-Host "`n=== Tier 3: Python & Data (2 servers) ==="
Download-Npm "pyright" "pyright" "1.1.411"
Download-Npm "sql-language-server" "sql-language-server" "1.7.1" @("vscode-languageserver-protocol@3.16.0")

Write-Host "`n=== Tier 4: JVM Languages (3 servers) ==="

# 11. Java - jdtls (platform-independent JVM tarball)
# The milestone registry.json pins for jdtls (direct installs fetch the same),
# checked against the SHA-256 Eclipse publishes beside it. A build of another
# version is replaced, and a failed download leaves nothing to package.
$jdtlsVer = "1.61.0"
$jdtlsDir = Join-Path $LspDir "jdtls"
if ((Test-Path (Join-Path $jdtlsDir "bin\jdtls")) -and (Test-PinnedVersion (Join-Path $jdtlsDir ".version") $jdtlsVer)) {
    Already-Present "jdtls"
} else {
    Write-Host "[jdtls] Downloading Eclipse JDT LS"
    if (Test-Path $jdtlsDir) { Remove-Item -Recurse -Force $jdtlsDir }
    New-Item -ItemType Directory -Force -Path $jdtlsDir | Out-Null
    $tgz = Join-Path $jdtlsDir "jdtls.tar.gz"
    if (Get-Verified "https://download.eclipse.org/jdtls/milestones/$jdtlsVer/jdt-language-server-$jdtlsVer-202609031315.tar.gz" $tgz) {
        # Windows' own tar, by its path: a GNU tar earlier on PATH (Git's,
        # where Git Bash started this script) takes the C: of the archive's
        # path for a host to connect to.
        $tar = Join-Path $env:SystemRoot "System32\tar.exe"
        $untarOutput = & $tar -xzf $tgz -C $jdtlsDir 2>&1 | Out-String
        $untarred = $LASTEXITCODE -eq 0
        Remove-Item $tgz -ErrorAction SilentlyContinue
        if ($untarred) {
            Set-Content -Path (Join-Path $jdtlsDir ".version") -Value $jdtlsVer -Encoding ASCII
            Mark-Downloaded "jdtls" $jdtlsVer
        } else {
            Remove-Item -Recurse -Force $jdtlsDir -ErrorAction SilentlyContinue
            Mark-Failed "jdtls" "extract failed: $($untarOutput.Trim())"
        }
    } else {
        Remove-Item -Recurse -Force $jdtlsDir -ErrorAction SilentlyContinue
        Mark-Failed "jdtls" "download failed"
    }
}

# 12. Kotlin - kotlin-lsp (JetBrains' Kotlin LSP)
# The standalone server brings its own Java runtime. The build is the one
# registry.json pins for kotlin-lsp (direct installs fetch the same), checked
# against the SHA-256 JetBrains publishes beside it. A build of another version
# is replaced, and a kotlin-language-server install, a server Devscriptor does
# not use, is removed.
$kotlinVer = "263.6379.0"
$kotlinDir = Join-Path $LspDir "kotlin-lsp"
if ((Test-Path (Join-Path $kotlinDir "bin\intellij-server.exe")) -and (Test-PinnedVersion (Join-Path $kotlinDir ".version") $kotlinVer)) {
    Already-Present "kotlin-lsp"
} else {
    $retired = Join-Path $LspDir "kotlin-language-server"
    if (Test-Path $retired) { Remove-Item -Recurse -Force $retired }
    Write-Host "[kotlin-lsp] Downloading JetBrains' Kotlin LSP $kotlinVer"
    $zip = Join-Path ([System.IO.Path]::GetTempPath()) "kotlin-lsp-$kotlinVer.win.zip"
    if (Get-Verified "https://download.jetbrains.com/language-server/kotlin-server/$kotlinVer/kotlin-server-$kotlinVer.win.zip" $zip) {
        if (Test-Path $kotlinDir) { Remove-Item -Recurse -Force $kotlinDir }
        # The Windows archive has no top-level folder: it unpacks as `kotlin-lsp\`.
        Expand-Archive -Path $zip -DestinationPath $kotlinDir -Force
        Set-Content -Path (Join-Path $kotlinDir ".version") -Value $kotlinVer -Encoding ASCII
        Mark-Downloaded "kotlin-lsp" $kotlinVer
    } else {
        Mark-Failed "kotlin-lsp" "download failed"
    }
    Remove-Item $zip -ErrorAction SilentlyContinue
}

# 13. Scala - metals (via coursier; JVM)
# A Coursier launcher for the build registry.json pins (direct installs make
# the same). .version records the build, and a launcher of another version is
# replaced once the new one is in.
$metalsVer = "1.6.7"
$metalsDir = Join-Path $LspDir "metals"
# The launcher holds Metals and the libraries it runs on (`--standalone`): a
# thin one, which would fetch them into Coursier's cache at its first start,
# ~130 MB that need the network, is replaced too.
$metalsJar = "coursier/bootstrap/launcher/jars/metals_2.13-$metalsVer.jar"
if ((Test-Path (Join-Path $metalsDir "bin\metals.bat")) -and (Test-PinnedVersion (Join-Path $metalsDir ".version") $metalsVer) -and
    (Test-FileHolds (Join-Path $metalsDir "bin\metals") $metalsJar)) {
    Already-Present "metals"
} else {
    $csBin = Find-Coursier
    if ($csBin) {
        Write-Host "[metals] Installing metals $metalsVer via coursier ($csBin)"
        $staging = New-Staging $metalsDir
        $mbin = Join-Path $staging "bin"
        New-Item -ItemType Directory -Force -Path $mbin | Out-Null
        $csErr = [System.IO.Path]::GetTempFileName()
        # Coursier writes its launcher to the name given and, on Windows, the
        # batch file that runs it beside it with `.bat` added: given
        # `metals.bat`, the batch file would be `metals.bat.bat`, and `metals.bat`,
        # which Devscriptor starts, the POSIX shell launcher.
        & $csBin bootstrap --standalone "org.scalameta:metals_2.13:$metalsVer" -o (Join-Path $mbin "metals") 2>$csErr | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Set-Content -Path (Join-Path $staging ".version") -Value $metalsVer -Encoding ASCII
            Install-Swapped $metalsDir
            Mark-Downloaded "metals" "coursier:$metalsVer"
        } else {
            Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
            Mark-Failed "metals" "coursier failed"
            Write-Host "coursier output:"
            Get-Content $csErr -Tail 20 | ForEach-Object { Write-Host "  $_" }
        }
        Remove-Item $csErr -ErrorAction SilentlyContinue
    } else {
        Mark-Failed "metals" "coursier not available"
    }
}

Write-Host "`n=== Tier 5: Microsoft Ecosystem (2 servers) ==="
Download-Dotnet "csharp-ls" "csharp-ls" "0.16.0"

Download-Dotnet "fsautocomplete" "fsautocomplete" "0.83.0"

Write-Host "`n=== Tier 6: Mobile, Systems & Other (5 servers) ==="

# 16. Swift - sourcekit-lsp: not packaged. It comes with the Swift toolchain
# (registry.json says `system`), and Devscriptor runs the toolchain's own: a
# copy works only with the compiler, SDK and DLLs of the toolchain it came
# from. An earlier download's copy is removed so that it is not packaged.
$sourcekitDir = Join-Path $LspDir "sourcekit-lsp"
if (Test-Path $sourcekitDir) { Remove-Item -Recurse -Force $sourcekitDir }
Write-Host "[sourcekit-lsp] Not packaged: it comes with the Swift toolchain"

# 17. Dart - dart-ls (the official Dart SDK)
#
# The directory MUST be named `dart-ls`: the resolver looks under
# <install-root>/<server-id>/, and `dart-ls` is the id in registry.json.
#
# `dart-ls\` *is* a Dart SDK root, so `dart-ls\bin\dart.exe language-server`
# runs without any Dart on the user's machine. Neither a wrapper around the
# build machine's `dart` nor a bare analysis_server.dart.snapshot would do: the
# wrapper needs that `dart`, and the snapshot only runs on the exact VM that
# built it and needs that VM's lib/ to resolve dart:core.
#
# The SDK is fetched from Google's archive and its published SHA-256 is checked.
# The build is the one registry.json pins for dart-ls, which direct installs
# fetch too; set $env:DART_SDK_VERSION to package another.
$dartPlatform = "windows-x64"
$dartArchive = "https://storage.googleapis.com/dart-archive/channels/stable/release"

$dartVersion = $env:DART_SDK_VERSION
if (-not $dartVersion) { $dartVersion = "3.12.2" }
if ((Test-Path (Join-Path $LspDir "dart-ls\bin\dart.exe")) -and (Test-PinnedVersion (Join-Path $LspDir "dart-ls\.version") $dartVersion)) {
    Already-Present "dart-ls"
} else {
    $zipUrl = "$dartArchive/$dartVersion/sdk/dartsdk-$dartPlatform-release.zip"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    $zipPath = Join-Path $tmp "dartsdk.zip"
    Write-Host "[dart-ls] Downloading Dart SDK $dartVersion ($dartPlatform)"

    $ok = $true
    try {
        Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing
    } catch {
        $ok = $false
        Mark-Failed "dart-ls" "download failed"
    }

    if ($ok) {
        # Google publishes a SHA-256 next to every archive. A truncated or
        # tampered 200 MB download must never be unpacked and shipped, nor one
        # whose published checksum could not be fetched.
        #
        # The sidecar is served as application/octet-stream, so Windows
        # PowerShell hands `.Content` back as a Byte[]; `-split` then
        # stringifies it to space-joined byte *values* and the first token
        # becomes "55" (the decimal for the hash's leading '7'). Download it
        # to a file and read it as text, exactly like the zip.
        $expected = $null
        try {
            $sumPath = Join-Path $tmp "dartsdk.zip.sha256sum"
            Invoke-WebRequest -Uri "$zipUrl.sha256sum" -OutFile $sumPath -UseBasicParsing
            $expected = ((Get-Content $sumPath -Raw) -split '\s+', 2)[0].Trim()
        } catch { }
        $actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()

        if (-not $expected -or ($expected.ToLower() -ne $actual)) {
            Mark-Failed "dart-ls" "checksum mismatch"
            Write-Host "[dart-ls] FAILED - SHA-256 mismatch (expected $expected, got $actual)"
        } else {
            Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
            $target = Join-Path $LspDir "dart-ls"
            if (Test-Path $target) { Remove-Item -Recurse -Force $target }
            # The archive holds a single `dart-sdk\` root; `dart-ls\` becomes it.
            Move-Item (Join-Path $tmp "dart-sdk") $target
            Set-Content -Path (Join-Path $target ".version") -Value $dartVersion -Encoding ASCII
            Mark-Downloaded "dart-ls" $dartVersion
        }
    }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# 18. Ruby - ruby-lsp
# `ruby-lsp-launcher` is the binstub the registry points at; the plain
# `ruby-lsp` binstub shells out to bundler and needs a Gemfile. The version is
# the one registry.json pins; a bundle of another version is replaced, as is one
# built for another Ruby than this machine's. Its native extensions are built
# for the Ruby that installs it, and Devscriptor installs it again, with the
# user's Ruby, wherever that is another one.
$RubyLspVersion = "0.26.11"
$RubyLspDir = Join-Path $LspDir "ruby-lsp"
$RubyLspRecorded = Get-Content (Join-Path $RubyLspDir ".version") -ErrorAction SilentlyContinue
if ((Test-Path (Join-Path $RubyLspDir "bin\ruby-lsp-launcher.bat")) -and ($RubyLspRecorded -eq $RubyLspVersion) -and
    (Test-BuiltForThisRuby $RubyLspDir)) {
    Already-Present "ruby-lsp"
} else {
    Download-Gem "ruby-lsp" "ruby-lsp" $RubyLspVersion
    # Only an install this run made: one that failed left the earlier bundle.
    if ($script:Downloaded.ContainsKey("ruby-lsp")) {
        Set-Content -Path (Join-Path $RubyLspDir ".version") -Value $RubyLspVersion -Encoding ASCII
    }
}

# 19. PHP - phpactor (platform-independent .phar + .cmd launcher)
if (Test-Path (Join-Path $LspDir "phpactor\bin\phpactor.cmd")) {
    Already-Present "phpactor"
} else {
    Write-Host "[phpactor] Downloading the latest release's phpactor.phar"
    $pbin = Join-Path $LspDir "phpactor\bin"
    New-Item -ItemType Directory -Force -Path $pbin | Out-Null
    if (Get-ReleaseAsset "phpactor/phpactor" '^phpactor\.phar$' (Join-Path $pbin "phpactor.phar")) {
        $cmd = "@echo off`r`nphp `"%~dp0phpactor.phar`" %*`r`n"
        Set-Content -Path (Join-Path $pbin "phpactor.cmd") -Value $cmd -Encoding ASCII
        Mark-Downloaded "phpactor" "github:phpactor/phpactor"
    } else {
        Remove-Item -Recurse -Force (Join-Path $LspDir "phpactor") -ErrorAction SilentlyContinue
        Mark-Failed "phpactor" "could not fetch a verified phpactor.phar"
    }
}

# 20. Bash - bash-language-server
Download-Npm "bash-language-server" "bash-language-server" "5.6.0"

# 21. Solidity - nomicfoundation-solc
Download-Npm "nomicfoundation-solc" "@nomicfoundation/solidity-language-server" "0.8.25"

Write-Host "`n==============================================="
Write-Host "Download Summary"
Write-Host "==============================================="
Write-Host ""
Write-Host "Successfully downloaded: $($script:Downloaded.Count)"
$script:Downloaded.GetEnumerator() | ForEach-Object { Write-Host "  OK  $($_.Key) ($($_.Value))" }
Write-Host ""
Write-Host "Failed/Skipped: $($script:Failed.Count)"
$script:Failed.GetEnumerator() | ForEach-Object { Write-Host "  XX  $($_.Key) - $($_.Value)" }
Write-Host ""
$size = (Get-ChildItem $LspDir -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
Write-Host ("Total size: {0:N1} MB" -f ($size / 1MB))
Write-Host ""
Write-Host "==============================================="
Write-Host "LSP Server download complete"
Write-Host "==============================================="
