<#
.SYNOPSIS
    Install dependencies for 21 LSP servers - Windows x86_64.
.DESCRIPTION
    Installs the language runtimes/SDKs the bundled LSP servers need, using winget
    (built into Windows 10/11). Run in an elevated PowerShell (Run as Administrator)
    so winget can install machine-wide. Re-run after installs so new PATH entries
    are picked up in a fresh shell.
.NOTES
    PHP is not installed: phpactor, the PHP server, needs PHP's posix
    extension, which PHP on Windows does not have.

    If PowerShell refuses to run this script with an execution-policy error, run:
      Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
#>

$ErrorActionPreference = 'Continue'

Write-Host "=============================================="
Write-Host "Installing Dependencies for LSP Servers"
Write-Host "Platform: Windows x86_64"
Write-Host "Languages: 21 supported"
Write-Host "=============================================="
Write-Host ""

function Test-Cmd([string]$name) {
    $null -ne (Get-Command $name -ErrorAction SilentlyContinue)
}

# Install a winget package only if its primary command is missing.
function Install-WinGet([string]$id, [string]$cmd, [string]$label = $null) {
    if (-not $label) { $label = $id }
    if ($cmd -and (Test-Cmd $cmd)) {
        Write-Host "  [OK] $label already installed"
        return
    }
    Write-Host "Installing $label ($id)..."
    winget install --id $id -e --source winget `
        --accept-package-agreements --accept-source-agreements --silent
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "winget could not install $label ($id). Install it manually if the server needs it."
    }
}

# RubyInstaller2 needs MSYS2 to build gems with native extensions (e.g. ruby-lsp).
function Ensure-Msys2ForRuby {
    if (-not (Test-Cmd ridk)) {
        Write-Warning "  ridk not found; skipping MSYS2 check (Ruby may not be RubyInstaller2)"
        return
    }

    $ridkInfo = & ridk version 2>&1 | Out-String
    if ($ridkInfo -match "msys2") {
        Write-Host "  [OK] MSYS2 recognized by RubyInstaller ridk"
        return
    }

    # Build a list of candidate MSYS2 roots. Ruby+Devkit bundles MSYS2 inside
    # the Ruby install directory, so check there first.
    $msys2Roots = [System.Collections.Generic.List[string]]::new()
    if (Test-Cmd ruby) {
        $rubyExe = (Get-Command ruby).Source
        $rubyRoot = Split-Path (Split-Path $rubyExe) -Parent
        $msys2Roots.Add((Join-Path $rubyRoot "msys64"))
    }
    $msys2Roots.Add("C:\msys64")
    $msys2Roots.Add("$env:ProgramFiles\msys64")
    $msys2Roots.Add("$env:LOCALAPPDATA\Programs\msys64")

    function Add-Msys2ToPath($root, $envName) {
        $toolBin = Join-Path $root "$envName\bin"
        $usrBin = Join-Path $root "usr\bin"
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($userPath -notlike "*$toolBin*") {
            [Environment]::SetEnvironmentVariable("Path", "$userPath;$toolBin;$usrBin", "User")
        }
        $env:Path = "$toolBin;$usrBin;$env:Path"
        Write-Host "  [OK] Added MSYS2 ($envName) to PATH: $root"
    }

    $found = $false
    foreach ($root in $msys2Roots) {
        if (-not (Test-Path (Join-Path $root "usr\bin\bash.exe"))) { continue }
        if (Test-Path (Join-Path $root "mingw64\bin\gcc.exe")) {
            Add-Msys2ToPath $root "mingw64"
            $found = $true
            break
        }
        if (Test-Path (Join-Path $root "ucrt64\bin\gcc.exe")) {
            Add-Msys2ToPath $root "ucrt64"
            $found = $true
            break
        }
    }

    if (-not $found) {
        Write-Host "  Installing MSYS2 via winget (required for Ruby native gems)..."
        winget install --id MSYS2.MSYS2 -e --source winget `
            --accept-package-agreements --accept-source-agreements --silent
        foreach ($root in $msys2Roots) {
            if (-not (Test-Path (Join-Path $root "usr\bin\bash.exe"))) { continue }
            if (Test-Path (Join-Path $root "mingw64\bin\gcc.exe")) {
                Add-Msys2ToPath $root "mingw64"
                $found = $true
                break
            }
            if (Test-Path (Join-Path $root "ucrt64\bin\gcc.exe")) {
                Add-Msys2ToPath $root "ucrt64"
                $found = $true
                break
            }
        }
    }

    if ($found) {
        # ridk install 1 initializes MSYS2 (GPG keys etc.), 2 updates, 3 installs
        # the MINGW/UCRT toolchain. With a fresh MSYS2 all three are needed.
        Write-Host "  Installing MSYS2/MINGW development toolchain via ridk..."
        & ridk install 1 2 3
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "ridk install returned an error; ruby-lsp may still fail."
        }
    } else {
        Write-Warning "MSYS2 not found. Ruby gems with native extensions (e.g. ruby-lsp) may fail to install."
    }
}

# 0. winget availability
Write-Host "[1/9] Checking winget..."
if (-not (Test-Cmd winget)) {
    Write-Error "winget is required. Install 'App Installer' from the Microsoft Store, then re-run."
    exit 1
}
Write-Host "  [OK] winget present"

# 1. Java (jdtls, which needs 21 or newer, and metals)
Write-Host "[2/9] Checking Java..."
Install-WinGet "Microsoft.OpenJDK.21" "java" "OpenJDK 21"

# 2. Python (jdtls launcher)
Write-Host "[3/9] Checking Python..."
# The Microsoft Store puts a `python` alias on PATH where no Python is
# installed; it prints no version, only how to install one.
$pythonVersion = if (Test-Cmd python) { & python --version 2>$null } else { $null }
if ($pythonVersion -match '^Python 3') {
    Write-Host "  [OK] $pythonVersion already installed"
} else {
    Install-WinGet "Python.Python.3.12" "" "Python 3.12"
}

# 3. Go (gopls)
Write-Host "[4/9] Checking Go..."
Install-WinGet "GoLang.Go" "go" "Go"

# 4. Node.js + npm (9 npm-based servers)
Write-Host "[5/9] Checking Node.js..."
Install-WinGet "OpenJS.NodeJS.LTS" "node" "Node.js LTS"

# 5. .NET SDK (csharp-ls, fsautocomplete)
Write-Host "[6/9] Checking .NET SDK..."
Install-WinGet "Microsoft.DotNet.SDK.8" "dotnet" ".NET SDK 8"

# 6. Ruby (ruby-lsp)
# ruby-lsp depends on prism, which has a native C extension. On Windows that
# requires RubyInstaller2 + MSYS2/MINGW (the "DevKit"). If Ruby is already
# present without DevKit, we install MSYS2 separately and run ridk install.
Write-Host "[7/9] Checking Ruby..."
$rubyOk = Test-Cmd ruby
$devkitOk = $false
if ($rubyOk -and (Test-Cmd ridk)) {
    $devkitOk = (& ridk version 2>&1 | Out-String) -match "msys2"
}
if ($rubyOk -and $devkitOk) {
    Write-Host "  [OK] Ruby with DevKit already installed"
} elseif ($rubyOk -and -not $devkitOk) {
    Write-Warning "  Ruby is installed but MSYS2/DevKit was not detected; installing MSYS2 separately..."
    Ensure-Msys2ForRuby
} else {
    # No Ruby at all -> install the bundled Ruby+DevKit package.
    Install-WinGet "RubyInstallerTeam.RubyWithDevKit.3.3" "ruby" "Ruby 3.3 with DevKit"
    Ensure-Msys2ForRuby
}

# 7. Coursier (Scala/Metals) - no winget; fetch the official cs.exe launcher
Write-Host "[8/9] Checking Coursier (Scala)..."
if (Test-Cmd cs) {
    Write-Host "  [OK] Coursier already installed"
} else {
    try {
        $csDir = Join-Path $env:LOCALAPPDATA "Coursier\bin"
        New-Item -ItemType Directory -Force -Path $csDir | Out-Null
        $zip = Join-Path $env:TEMP "cs-win32.zip"
        Invoke-WebRequest -Uri "https://github.com/coursier/launchers/raw/master/cs-x86_64-pc-win32.zip" -OutFile $zip
        Expand-Archive -Path $zip -DestinationPath $env:TEMP -Force
        Move-Item -Force (Join-Path $env:TEMP "cs-x86_64-pc-win32.exe") (Join-Path $csDir "cs.exe")
        Remove-Item $zip -ErrorAction SilentlyContinue
        # Add to user PATH for future shells
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($userPath -notlike "*$csDir*") {
            [Environment]::SetEnvironmentVariable("Path", "$userPath;$csDir", "User")
        }
        $env:Path = "$env:Path;$csDir"
        Write-Host "  [OK] Coursier installed to $csDir"
    } catch {
        Write-Warning "Could not install Coursier automatically: $_"
        Write-Warning "Install manually from https://get-coursier.io/ (metals will be skipped otherwise)."
    }
}

# 8. PHP - not needed: phpactor, the PHP server, refuses to start without PHP's
# posix extension, which PHP on Windows does not have, so it is not installed
# on Windows.
Write-Host "[9/9] PHP: not needed (phpactor needs PHP's posix extension, which PHP on Windows does not have)"

# Rust (optional - rust-analyzer can be auto-downloaded)
Write-Host ""
if (Test-Cmd rustup) {
    Write-Host "  [OK] Rust (rustup) detected"
} else {
    Write-Host "  [!] Rust not detected. rust-analyzer will be auto-downloaded."
    Write-Host "      To install Rust: winget install --id Rustlang.Rustup -e"
}

Write-Host ""
Write-Host "=============================================="
Write-Host "Dependencies Installation Complete!"
Write-Host "=============================================="
Write-Host ""
Write-Host "NOTE: open a NEW PowerShell window so freshly-installed tools are on PATH."
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Run: .\download-lsp-servers.ps1"
Write-Host "  2. Run: .\package-lsp-servers.ps1"
Write-Host "The LSP mirror workflow runs both on GitHub's runners and publishes the"
Write-Host "packages (docs/building.md)."
Write-Host ""
