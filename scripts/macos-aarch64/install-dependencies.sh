#!/usr/bin/env bash
# Install dependencies for 21 LSP servers - macOS aarch64 (Apple Silicon)
# Run this script as your normal user (NOT with sudo — Homebrew refuses root).
# This script checks if dependencies are already installed before installing.

set -e
set -o pipefail

echo "=============================================="
echo "Installing Dependencies for LSP Servers"
echo "Platform: macOS aarch64 (Apple Silicon)"
echo "Languages: 21 supported"
echo "=============================================="
echo ""

# Homebrew refuses to run as root; bail out early if invoked with sudo.
if [ "$EUID" -eq 0 ]; then
    echo "Please run as your normal user, NOT with sudo (Homebrew will refuse root)."
    exit 1
fi

# Helper function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Install a brew formula only if its primary command is missing.
brew_install_if_missing() {
    local cmd="$1"; shift
    local formula="${1:-$cmd}"
    if command_exists "$cmd"; then
        echo "  ✓ $cmd already installed"
    else
        echo "Installing $formula..."
        brew install "$formula"
    fi
}

echo "[1/8] Checking Homebrew..."
if ! command_exists brew; then
    echo "Homebrew is required. Install it first:"
    echo '  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"'
    exit 1
fi
echo "  ✓ Homebrew present ($(brew --version | head -1))"
BREW_PREFIX="$(brew --prefix)"

# Xcode Command Line Tools provide clang, git and make.
echo "[2/8] Checking Xcode Command Line Tools..."
if ! xcode-select -p >/dev/null 2>&1; then
    echo "Installing Xcode Command Line Tools (a GUI prompt may appear)..."
    xcode-select --install || true
    echo "  ⚠ Re-run this script once the Command Line Tools finish installing."
else
    echo "  ✓ Command Line Tools present ($(xcode-select -p))"
fi

# Bash 4 or newer for download-lsp-servers.sh, which uses `declare -A`: the
# bash on PATH, or Homebrew's, which the script runs itself again under. macOS's
# own is 3.2.
echo "[3/8] Checking modern bash (>= 4) for the other scripts..."
if bash -c '[ "${BASH_VERSINFO[0]}" -ge 4 ]' 2>/dev/null; then
    echo "  ✓ bash already installed ($(bash -c 'echo "$BASH_VERSION"'))"
elif [ -x "${BREW_PREFIX}/bin/bash" ]; then
    echo "  ✓ Homebrew's bash already installed"
else
    echo "Installing bash..."
    brew install bash
fi

# Basic tools
echo "[4/8] Checking basic tools (wget, jq, xz, unzip)..."
brew_install_if_missing wget wget
brew_install_if_missing jq jq
brew_install_if_missing xz xz
command_exists unzip && echo "  ✓ unzip already installed" || brew install unzip || true

# Java (for jdtls, which needs 21 or newer, and metals)
echo "[5/8] Checking Java..."
if ! command_exists java || ! java -version >/dev/null 2>&1; then
    echo "Installing OpenJDK 21..."
    brew install openjdk@21
    # openjdk is keg-only; symlink so the system Java wrappers can find it.
    sudo ln -sfn "${BREW_PREFIX}/opt/openjdk@21/libexec/openjdk.jdk" \
        /Library/Java/JavaVirtualMachines/openjdk-21.jdk 2>/dev/null || \
        echo "  ⚠ Add ${BREW_PREFIX}/opt/openjdk@21/bin to your PATH"
else
    echo "  ✓ Java already installed ($(java -version 2>&1 | head -1))"
fi

# Python (for jdtls launcher)
echo "[6/8] Checking Python..."
if ! command_exists python3; then
    brew install python
else
    echo "  ✓ Python already installed ($(python3 --version))"
fi

# Language toolchains for the remaining servers
echo "[7/8] Checking language toolchains (Go, PHP, Ruby, Node, Coursier)..."
brew_install_if_missing go go
brew_install_if_missing php php
# Ruby 2.7 or newer for ruby-lsp, on PATH or Homebrew's, where
# download-lsp-servers.sh looks: macOS's own /usr/bin/ruby is older.
if command_exists ruby && ruby -e 'exit(RUBY_VERSION >= "2.7.0" ? 0 : 1)' 2>/dev/null; then
    echo "  ✓ ruby already installed ($(ruby --version))"
elif [ -x "${BREW_PREFIX}/opt/ruby/bin/ruby" ]; then
    echo "  ✓ Homebrew's ruby already installed"
else
    echo "Installing ruby..."
    brew install ruby
fi
brew_install_if_missing node node
brew_install_if_missing cs coursier

# .NET SDK (for csharp-ls and fsautocomplete) — cask
echo "[8/8] Checking .NET SDK..."
if ! command_exists dotnet; then
    echo "Installing .NET SDK..."
    brew install --cask dotnet-sdk
else
    echo "  ✓ .NET SDK already installed ($(dotnet --version))"
fi

# Rust (optional — rust-analyzer can be auto-downloaded)
if command_exists rustup; then
    echo "  ✓ Rust (rustup) detected"
else
    echo "  ⚠ Rust not detected. rust-analyzer will be auto-downloaded."
    echo "     To install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi

echo ""
echo "=============================================="
echo "Dependencies Installation Complete!"
echo "=============================================="
echo ""
echo "Installed/Detected:"
command_exists java   && echo "  ✓ Java ($(java -version 2>&1 | head -1 | cut -d'"' -f2 || true))"
command_exists python3 && echo "  ✓ Python ($(python3 --version 2>&1 | cut -d' ' -f2))"
command_exists go     && echo "  ✓ Go ($(go version | awk '{print $3}'))"
command_exists php    && echo "  ✓ PHP ($(php --version | head -1 | awk '{print $2}'))"
command_exists ruby   && echo "  ✓ Ruby ($(ruby --version | awk '{print $2}'))"
command_exists node   && echo "  ✓ Node.js ($(node --version))"
command_exists dotnet && echo "  ✓ .NET SDK ($(dotnet --version))"
command_exists cs     && echo "  ✓ Coursier (Scala)"
command_exists rustup && echo "  ✓ Rust"
echo ""
echo "Next steps:"
echo "  1. Run: ./download-lsp-servers.sh"
echo "  2. Run: ./package-lsp-servers.sh"
echo "The LSP mirror workflow runs both on GitHub's runners and publishes the"
echo "packages (docs/building.md)."
echo ""
