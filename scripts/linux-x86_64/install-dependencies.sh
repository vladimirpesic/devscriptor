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

# Whether the version $1 is at least $2, both as major[.minor]: a server's
# runtime is checked for the version it needs, not only for being there.
version_at_least() {
    local have_major have_minor want_major want_minor
    IFS=. read -r have_major have_minor _ <<< "$1"
    IFS=. read -r want_major want_minor _ <<< "$2"
    have_major=${have_major//[!0-9]/}; have_minor=${have_minor//[!0-9]/}
    want_minor=${want_minor:-0}
    have_major=${have_major:-0}; have_minor=${have_minor:-0}
    [ "$have_major" -gt "$want_major" ] ||
        { [ "$have_major" -eq "$want_major" ] && [ "$have_minor" -ge "$want_minor" ]; }
}

# The major version of the `java` on the PATH (8 for Java 8's "1.8.0"), or
# nothing.
java_major() {
    java -version 2>&1 | sed -n 's/.*version "\(1\.\)\{0,1\}\([0-9][0-9]*\).*/\2/p' | head -n 1
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
if command_exists java && version_at_least "$(java_major)" 21; then
    echo "  ✓ Java already installed ($(java -version 2>&1 | head -1))"
else
    command_exists java && echo "  Java $(java_major) is older than the 21 jdtls needs"
    echo "Installing OpenJDK 21..."
    apt install -y openjdk-21-jdk ||
        echo "  ⚠ openjdk-21-jdk is not in this distribution: install Java 21 or newer from https://adoptium.net/"
    if command_exists java && ! version_at_least "$(java_major)" 21; then
        echo "  ⚠ java on the PATH is still Java $(java_major): choose 21 with \`update-alternatives --config java\`, or set JAVA_HOME to it (jdtls takes JAVA_HOME's Java first)"
    fi
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

# Install PHP (for phpactor), with the mbstring extension phpactor refuses to
# start without, which Debian's and Ubuntu's php-cli does not include.
echo "[6/7] Checking PHP..."
if ! command_exists php; then
    echo "Installing PHP..."
    apt install -y php-cli php-mbstring
elif ! php -r 'exit(extension_loaded("mbstring") ? 0 : 1);' 2>/dev/null; then
    echo "Installing PHP's mbstring extension..."
    apt install -y php-mbstring
fi
PHP_VERSION_HERE=$(php -r 'echo PHP_VERSION;' 2>/dev/null || true)
if version_at_least "$PHP_VERSION_HERE" 8.2; then
    echo "  ✓ PHP already installed ($(php --version | head -1))"
elif [ -n "$PHP_VERSION_HERE" ]; then
    echo "  ⚠ PHP $PHP_VERSION_HERE is older than the 8.2 phpactor needs: install PHP 8.2 or newer (Ubuntu 22.04: the ondrej/php PPA)"
else
    echo "  ⚠ PHP was not installed: phpactor needs PHP 8.2 or newer"
fi

# Install Ruby (for ruby-lsp)
echo "[7/7] Checking Ruby..."
if ! command_exists ruby; then
    echo "Installing Ruby..."
    apt install -y ruby ruby-dev
fi
RUBY_VERSION_HERE=$(ruby -e 'print RUBY_VERSION' 2>/dev/null || true)
if version_at_least "$RUBY_VERSION_HERE" 3.0; then
    echo "  ✓ Ruby already installed ($(ruby --version))"
elif [ -n "$RUBY_VERSION_HERE" ]; then
    echo "  ⚠ Ruby $RUBY_VERSION_HERE is older than the 3.0 ruby-lsp needs: install Ruby 3.0 or newer"
else
    echo "  ⚠ Ruby was not installed: ruby-lsp needs Ruby 3.0 or newer"
fi

# Install npm/node if not present
echo ""
echo "Checking language-specific package managers..."

if command_exists npm && command_exists node && version_at_least "$(node --version)" 18; then
    echo "  ✓ Node.js/npm already installed ($(node --version))"
else
    command_exists node && echo "  Node.js $(node --version) is older than the 18 the npm servers need"
    echo "Installing Node.js and npm..."
    curl -fsSL https://deb.nodesource.com/setup_24.x | bash -
    apt install -y nodejs
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
