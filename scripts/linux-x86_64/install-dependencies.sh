#!/bin/bash
# Install dependencies for 21 LSP servers - Linux x86_64
# Run this script with sudo or as root
# This script checks if dependencies are already installed before installing

set -e
set -o pipefail

echo "=============================================="
echo "Installing Dependencies for LSP Servers"
echo "Platform: Linux x86_64"
echo "Languages: 21 supported"
echo "=============================================="
echo ""

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo "Please run as root or with sudo"
    exit 1
fi

# Helper function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Whether the apt package is installed.
apt_package_installed() {
    dpkg -l "$1" 2>/dev/null | grep -q "^ii"
}

echo "[1/7] Updating package lists..."
apt update || true

# Install basic build tools
echo "[2/7] Checking basic build tools..."
BASIC_TOOLS="curl wget build-essential pkg-config libssl-dev libcurl4-openssl-dev unzip xz-utils git jq"
TOOLS_TO_INSTALL=""
for tool in $BASIC_TOOLS; do
    if ! apt_package_installed "$tool" 2>/dev/null; then
        TOOLS_TO_INSTALL="$TOOLS_TO_INSTALL $tool"
    fi
done
if [ -n "$TOOLS_TO_INSTALL" ]; then
    echo "Installing missing tools:$TOOLS_TO_INSTALL"
    apt install -y $TOOLS_TO_INSTALL
else
    echo "  ✓ All basic tools already installed"
fi

# Install Java (for jdtls, which needs 21 or newer, and metals)
echo "[3/7] Checking Java..."
if ! command_exists java; then
    echo "Installing OpenJDK 21..."
    apt install -y openjdk-21-jdk
else
    echo "  ✓ Java already installed ($(java -version 2>&1 | head -1))"
fi

# Install Python (for jdtls launcher)
echo "[4/7] Checking Python..."
if ! command_exists python3; then
    echo "Installing Python..."
    apt install -y python3
else
    echo "  ✓ Python already installed ($(python3 --version))"
fi

# Install Go (for gopls)
echo "[5/7] Checking Go..."
if ! command_exists go; then
    echo "Installing Go..."
    apt install -y golang-go
else
    echo "  ✓ Go already installed ($(go version))"
fi

# Install PHP (for phpactor)
echo "[6/7] Checking PHP..."
if ! command_exists php; then
    echo "Installing PHP..."
    apt install -y php-cli
else
    echo "  ✓ PHP already installed ($(php --version | head -1))"
fi

# Install Ruby (for ruby-lsp)
echo "[7/7] Checking Ruby..."
if ! command_exists ruby; then
    echo "Installing Ruby..."
    apt install -y ruby ruby-dev
else
    echo "  ✓ Ruby already installed ($(ruby --version))"
fi

# Install npm/node if not present
echo ""
echo "Checking language-specific package managers..."

if ! command_exists npm; then
    echo "Installing Node.js and npm..."
    curl -fsSL https://deb.nodesource.com/setup_24.x | bash -
    apt install -y nodejs
else
    echo "  ✓ Node.js/npm already installed ($(node --version))"
fi

# Install .NET SDK (for csharp-ls and fsautocomplete)
if ! command_exists dotnet; then
    echo "Installing .NET SDK..."
    UBUNTU_VERSION=$(lsb_release -rs 2>/dev/null || echo "22.04")
    wget https://packages.microsoft.com/config/ubuntu/${UBUNTU_VERSION}/packages-microsoft-prod.deb -O packages-microsoft-prod.deb
    dpkg -i packages-microsoft-prod.deb
    rm packages-microsoft-prod.deb
    apt update || true
    apt install -y dotnet-sdk-8.0
else
    echo "  ✓ .NET SDK already installed ($(dotnet --version))"
fi

# Install Coursier (for Scala/Metals)
if ! command_exists cs; then
    echo "Installing Coursier (Scala package manager)..."
    curl -fL https://github.com/coursier/launchers/raw/master/cs-x86_64-pc-linux.gz | gzip -d > /usr/local/bin/cs
    chmod +x /usr/local/bin/cs
else
    echo "  ✓ Coursier already installed"
fi

# Check for Rust (optional, rust-analyzer can be auto-downloaded)
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
command_exists java && echo "  ✓ OpenJDK ($(java -version 2>&1 | head -1 | cut -d'"' -f2 || true))"
command_exists python3 && echo "  ✓ Python ($(python3 --version 2>&1 | cut -d' ' -f2))"
command_exists go && echo "  ✓ Go ($(go version | awk '{print $3}'))"
command_exists php && echo "  ✓ PHP ($(php --version | head -1 | awk '{print $2}'))"
command_exists ruby && echo "  ✓ Ruby ($(ruby --version | awk '{print $2}'))"
command_exists node && echo "  ✓ Node.js ($(node --version))"
command_exists dotnet && echo "  ✓ .NET SDK ($(dotnet --version))"
command_exists cs && echo "  ✓ Coursier (Scala)"
command_exists rustup && echo "  ✓ Rust"
echo ""
echo "Next steps:"
echo "  1. Run: ./download-lsp-servers.sh"
echo "  2. Run: ./package-lsp-servers.sh"
echo "The LSP mirror workflow runs both on GitHub's runners and publishes the"
echo "packages (docs/building.md)."
echo ""
