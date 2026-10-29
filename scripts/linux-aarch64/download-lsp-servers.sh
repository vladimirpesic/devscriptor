#!/bin/bash
# Download script for 20 LSP servers - Linux ARM64 (aarch64)
# This script downloads LSP servers for all supported languages

set -e
set -o pipefail

# A failing command substitution ends the script under `set -e`, so every
# lookup that may come back empty (a tool that is not installed, a release or
# checksum that cannot be fetched) ends in `|| true`: its server is reported as
# failed, and the run goes on to the next one.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LSP_DIR="$SCRIPT_DIR/lsp-servers"
mkdir -p "$LSP_DIR"

echo "==============================================="
echo "Downloading 20 LSP Servers"
echo "==============================================="
echo "Target directory: $LSP_DIR"
echo ""

# Track download status
declare -A DOWNLOADED
declare -A FAILED

# Helper function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

already_present() {
    local name=$1
    echo "[$name] Already present, skipping"
    DOWNLOADED[$name]="already present"
}

# download_npm <id> <package> <version> [<companion>...]: install <package> at
# <version>, the build registry.json pins (direct installs fetch the same), into
# lsp-servers/<id>/, with the companions registry.json lists for it: npm
# packages it needs but does not depend on (typescript-language-server and volar
# refuse to start without a `typescript` to point at; sql-language-server
# reaches into vscode-languageserver-protocol's internals, which newer versions
# hide behind an `exports` map). `.version` records the build, and an install
# of another version is replaced once the new one is in (see `swap_in`).
download_npm() {
    local name=$1 package=$2 version=$3
    shift 3
    local target_dir="$LSP_DIR/$name"
    if [ "$(cat "$target_dir/.version" 2>/dev/null)" = "$version" ]; then
        already_present "$name"
        return
    fi
    echo "[$name] Installing via npm: $package@$version $*"
    rm -rf "${target_dir:?}.new"
    mkdir -p "$target_dir.new"
    if (cd "$target_dir.new" && npm install --omit=dev "$package@$version" "$@" 2>&1 | tail -5); then
        printf '%s\n' "$version" > "$target_dir.new/.version"
        swap_in "$target_dir"
        DOWNLOADED[$name]="npm:$package@$version"
        echo "[$name] ✓ Downloaded successfully"
    else
        rm -rf "${target_dir:?}.new"
        FAILED[$name]="npm failed"
        echo "[$name] ✗ Failed to download"
    fi
}

# swap_in <dir>: put the install made in `<dir>.new` in place of <dir>. A
# server is installed beside the one it replaces, so that an install that
# fails (no npm, no dotnet, no network) leaves the old one as it was.
swap_in() {
    rm -rf "${1:?}"
    mv "$1.new" "$1"
}

# sha256_of <file>: the file's SHA-256, by sha256sum or shasum.
sha256_of() {
    { sha256sum "$1" 2>/dev/null || shasum -a 256 "$1" 2>/dev/null; } | awk '{print $1}' || true
}

# fetch_verified <url> <dest>: download <url> to <dest> and check it against
# the SHA-256 its publisher puts beside it (<url>.sha256). A download whose
# checksum is missing or wrong is deleted, and the call fails.
fetch_verified() {
    local url=$1 dest=$2 expected actual
    curl -fsSL --max-redirs 10 -o "$dest" "$url" || { rm -f "$dest"; return 1; }
    expected=$(curl -fsSL "$url.sha256" | awk '{print $1}' || true)
    actual=$(sha256_of "$dest")
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        echo "  SHA-256 mismatch for $(basename "$url") (expected ${expected:-none fetched}, got $actual)"
        rm -f "$dest"
        return 1
    fi
}

# The token GitHub's API is asked with, where one is given (the LSP mirror
# workflow gives its own): without one, the API answers 60 requests an hour
# from an address, which CI runners share. It is taken out of the
# environment, so that no installer this script runs (npm, gem, go, dotnet)
# sees it.
GITHUB_API_TOKEN="${GITHUB_TOKEN:-}"
export -n GITHUB_API_TOKEN
unset GITHUB_TOKEN

# github_api <path>: GitHub's API answer for <path>, asked with the token where
# one is given, which reaches curl on its standard input, not its command line.
github_api() {
    if [ -n "$GITHUB_API_TOKEN" ]; then
        printf 'header = "Authorization: Bearer %s"\n' "$GITHUB_API_TOKEN" |
            curl -fsSL -K - "https://api.github.com/$1"
    else
        curl -fsSL "https://api.github.com/$1"
    fi
}

# fetch_release_asset <owner/repo> <name-regex> <dest>: download the asset of
# the repository's latest GitHub release whose name matches to <dest>, checked
# against the SHA-256 GitHub publishes for it, and print the asset's name. A
# release that cannot be read, no matching asset, no published checksum or a
# download that does not match it fails the call, and leaves nothing at <dest>.
fetch_release_asset() {
    local found name url expected actual
    found=$(github_api "repos/$1/releases/latest" | jq -r --arg re "$2" \
        'first(.assets[] | select(.name | test($re))) | "\(.name) \(.browser_download_url) \((.digest // "") | ltrimstr("sha256:"))"' || true)
    read -r name url expected <<< "$found"
    if [ -z "$url" ]; then
        echo "  no asset of $1's latest release matches $2 (or the release could not be read)" >&2
        return 1
    fi
    curl -fsSL --max-redirs 10 -o "$3" "$url" || { rm -f "$3"; return 1; }
    actual=$(sha256_of "$3")
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        echo "  SHA-256 mismatch for $name (expected ${expected:-none published}, got $actual)" >&2
        rm -f "$3"
        return 1
    fi
    echo "$name"
}

# Helper for go install
download_go() {
    local name=$1
    local package=$2
    local target_dir="$LSP_DIR/$name"

    echo "[$name] Installing via go: $package"
    mkdir -p "$target_dir/bin"

    local go_err
    go_err=$(mktemp)
    # Install into the target directory. Clearing GOOS/GOARCH avoids the
    # "cannot install cross-compiled binaries when GOBIN is set" error when
    # the caller has these variables set in their environment. On the native
    # aarch64 VM the host architecture is already arm64.
    if GOBIN="$target_dir/bin" GOOS="" GOARCH="" go install "$package@latest" >"$go_err" 2>&1; then
        if [ -x "$target_dir/bin/$name" ]; then
            DOWNLOADED[$name]="go:$package"
            echo "[$name] ✓ Installed successfully"
        else
            FAILED[$name]="go binary not found in $target_dir/bin"
            echo "[$name] ✗ Binary not found in $target_dir/bin"
            ls -la "$target_dir/bin" 2>/dev/null || true
        fi
    else
        FAILED[$name]="go install failed"
        echo "[$name] ✗ Failed to install"
        echo "go install output:"
        tail -20 "$go_err" | sed 's/^/  /'
    fi
    rm -f "$go_err"
}

# download_gem <id> <package> [<version>]: install the gem, and its executables
# into `bin/`, beside an earlier install, which it replaces once it is in (see
# `swap_in`).
download_gem() {
    local name=$1 package=$2 version=${3:-}
    local target_dir="$LSP_DIR/$name.new"

    echo "[$name] Installing via gem: $package"
    rm -rf "${target_dir:?}"
    mkdir -p "$target_dir/bin"

    local gem_err found_bin
    gem_err=$(mktemp)
    if gem install --install-dir="$target_dir" --bindir="$target_dir/bin" --no-document ${version:+--version "$version"} "$package" >"$gem_err" 2>&1; then
        if [ ! -x "$target_dir/bin/$name" ]; then
            # The binary may live inside the gem's bin/ directory.
            found_bin=$(find "$target_dir" -name "$name" -type f -perm -u+x 2>/dev/null | head -1 || true)
            [ -n "$found_bin" ] && cp "$found_bin" "$target_dir/bin/"
        fi
        if [ -x "$target_dir/bin/$name" ]; then
            swap_in "$LSP_DIR/$name"
            DOWNLOADED[$name]="gem:$package"
            echo "[$name] ✓ Installed successfully"
        else
            rm -rf "${target_dir:?}"
            FAILED[$name]="gem binary not found"
            echo "[$name] ✗ Gem binary not found"
        fi
    else
        rm -rf "${target_dir:?}"
        FAILED[$name]="gem failed"
        echo "[$name] ✗ Failed to install"
        echo "gem install output:"
        tail -20 "$gem_err" | sed 's/^/  /'
    fi
    rm -f "$gem_err"
}

# built_for_this_ruby <bundle>: whether the gem bundle's native extensions,
# under `extensions/<platform>/<api>/`, are this machine's Ruby's, as
# Devscriptor checks before it runs one: another Ruby misses the gems they
# belong to (Ruby 3.3 finds no `prism` in a bundle built with 3.2). A bundle
# with no native extensions runs on any.
built_for_this_ruby() {
    local api
    [ -d "$1/extensions" ] || return 0
    api=$(ruby -e 'print Gem.extension_api_version' 2>/dev/null || true)
    [ -n "$api" ] && [ -n "$(find "$1/extensions" -mindepth 2 -maxdepth 2 -type d -name "$api" 2>/dev/null | head -1 || true)" ]
}

# Helper to locate the Coursier launcher.
find_coursier() {
    if command_exists cs; then
        command -v cs
        return 0
    fi
    if command_exists coursier; then
        command -v coursier
        return 0
    fi
    local candidates=(
        "$HOME/.local/share/coursier/cs"
        "$HOME/.cargo/bin/cs"
        "/usr/local/bin/cs"
        "/usr/bin/cs"
        "/usr/local/bin/coursier"
        "/usr/bin/coursier"
    )
    for p in "${candidates[@]}"; do
        [ -x "$p" ] && { echo "$p"; return 0; }
    done
    return 1
}

# download_dotnet <id> <package> <version>: install the .NET tool at <version>,
# the build registry.json pins, into lsp-servers/<id>/bin/. `.version` records
# the build, and an install of another version is replaced once the new one is
# in (see `swap_in`). The dotnet CLI's first-run files go to a directory of
# their own, not into the package.
download_dotnet() {
    local name=$1 package=$2 version=$3
    local target_dir="$LSP_DIR/$name" cli_home
    if [ "$(cat "$target_dir/.version" 2>/dev/null)" = "$version" ]; then
        already_present "$name"
        return
    fi
    echo "[$name] Installing via dotnet: $package $version"
    rm -rf "${target_dir:?}.new"
    mkdir -p "$target_dir.new"
    cli_home=$(mktemp -d)
    if DOTNET_CLI_HOME="$cli_home" dotnet tool install --tool-path="$target_dir.new/bin" "$package" --version "$version" 2>&1 | tail -5; then
        printf '%s\n' "$version" > "$target_dir.new/.version"
        swap_in "$target_dir"
        DOWNLOADED[$name]="dotnet:$package@$version"
        echo "[$name] ✓ Installed successfully"
    else
        rm -rf "${target_dir:?}.new"
        FAILED[$name]="dotnet failed"
        echo "[$name] ✗ Failed to install"
    fi
    rm -rf "${cli_home:?}"
}

echo ""
echo "==============================================="
echo "Tier 1: Core Languages (4 servers)"
echo "==============================================="

# 1. Rust - rust-analyzer
#
# The binary must come from the upstream release, not from this machine:
#
#   * `~/.cargo/bin/rust-analyzer` is a rustup *proxy*. Copied elsewhere it dies
#     with "rustup could not choose a version of rust-analyzer to run".
#   * the toolchain's own `$(rustc --print sysroot)/bin/rust-analyzer` is linked
#     against `librustc_driver-*.so` inside that toolchain, so it cannot load
#     once copied out of it.
#
# The published `*-standalone` asset has neither problem.
RUST_ANALYZER_ASSET="rust-analyzer-aarch64-unknown-linux-gnu.gz"

if [ -e "$LSP_DIR/rust-analyzer/bin/rust-analyzer" ]; then
    already_present "rust-analyzer"
else
    echo "[rust-analyzer] Downloading the latest release's $RUST_ANALYZER_ASSET"
    ra_tmp=$(mktemp -d)
    if fetch_release_asset rust-lang/rust-analyzer "^${RUST_ANALYZER_ASSET//./\\.}\$" "$ra_tmp/ra.gz" >/dev/null; then
        mkdir -p "$LSP_DIR/rust-analyzer/bin"
        ra_bin="$LSP_DIR/rust-analyzer/bin/rust-analyzer"
        if gunzip -c "$ra_tmp/ra.gz" > "$ra_bin" 2>/dev/null; then
            chmod +x "$ra_bin"
            # A rustup proxy would fail here; the standalone build does not.
            if ra_ver="$("$ra_bin" --version 2>&1)"; then
                DOWNLOADED["rust-analyzer"]="github:rust-lang/rust-analyzer"
                echo "[rust-analyzer] OK ($ra_ver)"
            else
                FAILED["rust-analyzer"]="downloaded binary does not run"
                echo "[rust-analyzer] FAILED - the downloaded binary does not run:"
                printf '        %s\n' "$ra_ver"
            fi
        else
            FAILED["rust-analyzer"]="unpack failed"
            echo "[rust-analyzer] FAILED - could not unpack $RUST_ANALYZER_ASSET"
        fi
    else
        FAILED["rust-analyzer"]="release asset not fetched"
        echo "[rust-analyzer] FAILED - could not fetch a verified $RUST_ANALYZER_ASSET"
    fi
    rm -rf "$ra_tmp"
fi

# 2. C/C++ - clangd: not packaged. clangd publishes no Linux ARM build (its
# release has Linux x86_64, macOS and Windows builds; registry.json lists them
# in `published_for`), a distribution's clangd links the system's libclang-cpp
# and libLLVM and runs only where they are, and the `clangd` on PATH may be
# another program's proxy (swiftly puts itself there under that name).
# Devscriptor runs the system's (`apt install clangd`). An earlier download's
# copy is removed so that it is not packaged.
rm -rf "${LSP_DIR:?}/clangd"
echo "[clangd] Not packaged: clangd publishes no Linux ARM build; install the system's"

# 3. Go - gopls
if command_exists go; then
    if [ -e "$LSP_DIR/gopls/bin/gopls" ]; then
        already_present "gopls"
    else
        download_go "gopls" "golang.org/x/tools/gopls"
    fi
else
    FAILED["gopls"]="go not available"
    echo "[gopls] ✗ go not available"
fi

# 4. TypeScript/JavaScript - typescript-language-server
download_npm "typescript-language-server" "typescript-language-server" "5.3.0" "typescript@5.9.3"

echo ""
echo "==============================================="
echo "Tier 2: Web Frontend (4 servers)"
echo "==============================================="

# 5. Vue - volar
download_npm "volar" "@vue/language-server" "2.2.12" "typescript@5.9.3"

# 6. Svelte - svelte-language-server
download_npm "svelte-language-server" "svelte-language-server" "0.18.3"

# 7. HTML - html-languageserver
download_npm "html-languageserver" "vscode-langservers-extracted" "4.10.0"

# 8. GraphQL - graphql-language-service
download_npm "graphql-language-service" "graphql-language-service-cli" "3.5.0"

echo ""
echo "==============================================="
echo "Tier 3: Python & Data (2 servers)"
echo "==============================================="

# 9. Python - pyright
download_npm "pyright" "pyright" "1.1.411"

# 10. SQL - sql-language-server
download_npm "sql-language-server" "sql-language-server" "1.7.1" "vscode-languageserver-protocol@3.16.0"

echo ""
echo "==============================================="
echo "Tier 4: JVM Languages (3 servers)"
echo "==============================================="

# 11. Java - jdtls
# The milestone registry.json pins for jdtls (direct installs fetch the same),
# checked against the SHA-256 Eclipse publishes beside it. A build of another
# version is replaced, and a failed download leaves nothing to package.
JDTLS_VERSION="1.61.0"
if [ -e "$LSP_DIR/jdtls/bin/jdtls" ] && [ "$(cat "$LSP_DIR/jdtls/.version" 2>/dev/null)" = "$JDTLS_VERSION" ]; then
    already_present "jdtls"
else
    echo "[jdtls] Downloading Eclipse JDT LS"
    JDTLS_URL="https://download.eclipse.org/jdtls/milestones/${JDTLS_VERSION}/jdt-language-server-${JDTLS_VERSION}-202609031315.tar.gz"
    rm -rf "${LSP_DIR:?}/jdtls"
    mkdir -p "$LSP_DIR/jdtls"

    if fetch_verified "$JDTLS_URL" "$LSP_DIR/jdtls/jdtls.tar.gz" && \
       tar -xzf "$LSP_DIR/jdtls/jdtls.tar.gz" -C "$LSP_DIR/jdtls" && \
       rm -f "$LSP_DIR/jdtls/jdtls.tar.gz"; then
        printf '%s\n' "$JDTLS_VERSION" > "$LSP_DIR/jdtls/.version"
        DOWNLOADED["jdtls"]="$JDTLS_VERSION"
        echo "[jdtls] ✓ Downloaded successfully"
    else
        rm -rf "${LSP_DIR:?}/jdtls"
        FAILED["jdtls"]="download failed"
        echo "[jdtls] ✗ Failed to download"
    fi
fi

# 12. Kotlin - kotlin-lsp (JetBrains' Kotlin LSP)
# The standalone server brings its own Java runtime. The build is the one
# registry.json pins for kotlin-lsp (direct installs fetch the same), checked
# against the SHA-256 JetBrains publishes beside it. A build of another version
# is replaced, and a kotlin-language-server install, a server Devscriptor does
# not use, is removed.
KOTLIN_LSP_VERSION="263.6379.0"
if [ -e "$LSP_DIR/kotlin-lsp/bin/intellij-server" ] && [ "$(cat "$LSP_DIR/kotlin-lsp/.version" 2>/dev/null)" = "$KOTLIN_LSP_VERSION" ]; then
    already_present "kotlin-lsp"
else
    rm -rf "${LSP_DIR:?}/kotlin-language-server"
    KOTLIN_LSP_URL="https://download.jetbrains.com/language-server/kotlin-server/${KOTLIN_LSP_VERSION}/kotlin-server-${KOTLIN_LSP_VERSION}-aarch64.tar.gz"
    echo "[kotlin-lsp] Downloading JetBrains' Kotlin LSP $KOTLIN_LSP_VERSION"
    KOTLIN_TMP=$(mktemp -d)
    # The archive holds a single `kotlin-server-<version>/` root; `kotlin-lsp/` becomes it.
    if fetch_verified "$KOTLIN_LSP_URL" "$KOTLIN_TMP/kotlin-lsp-aarch64.tar.gz" && \
       tar -xzf "$KOTLIN_TMP/kotlin-lsp-aarch64.tar.gz" -C "$KOTLIN_TMP"; then
        rm -rf "${LSP_DIR:?}/kotlin-lsp"
        mkdir -p "$LSP_DIR"
        mv "$KOTLIN_TMP/kotlin-server-$KOTLIN_LSP_VERSION" "$LSP_DIR/kotlin-lsp"
        printf '%s\n' "$KOTLIN_LSP_VERSION" > "$LSP_DIR/kotlin-lsp/.version"
        DOWNLOADED["kotlin-lsp"]="$KOTLIN_LSP_VERSION"
        echo "[kotlin-lsp] ✓ Downloaded successfully"
    else
        FAILED["kotlin-lsp"]="download failed"
        echo "[kotlin-lsp] ✗ Failed to download"
    fi
    rm -rf "${KOTLIN_TMP:?}"
fi

# 13. Scala - metals
# A Coursier launcher for the build registry.json pins (direct installs make the
# same). `.version` records the build, and a launcher of another version is
# replaced once the new one is in. It holds Metals and the libraries it runs on
# (`--standalone`): a thin launcher, which would fetch them into Coursier's
# cache at its first start, ~130 MB that need the network, is replaced too.
METALS_VERSION="1.6.7"
METALS_JAR="coursier/bootstrap/launcher/jars/metals_2.13-$METALS_VERSION.jar"
if [ -e "$LSP_DIR/metals/bin/metals" ] && [ "$(cat "$LSP_DIR/metals/.version" 2>/dev/null)" = "$METALS_VERSION" ] &&
   grep -aqF "$METALS_JAR" "$LSP_DIR/metals/bin/metals"; then
    already_present "metals"
else
    CS_BIN="$(find_coursier || true)"
    if [ -n "$CS_BIN" ]; then
        echo "[metals] Installing metals $METALS_VERSION via coursier ($CS_BIN)"
        rm -rf "${LSP_DIR:?}/metals.new"
        mkdir -p "$LSP_DIR/metals.new/bin"
        cs_err=$(mktemp)
        if "$CS_BIN" bootstrap --standalone "org.scalameta:metals_2.13:$METALS_VERSION" -o "$LSP_DIR/metals.new/bin/metals" >"$cs_err" 2>&1; then
            printf '%s\n' "$METALS_VERSION" > "$LSP_DIR/metals.new/.version"
            swap_in "$LSP_DIR/metals"
            DOWNLOADED["metals"]="coursier:$METALS_VERSION"
            echo "[metals] ✓ Installed successfully"
        else
            rm -rf "${LSP_DIR:?}/metals.new"
            FAILED["metals"]="coursier failed"
            echo "[metals] ✗ Failed to install"
            echo "coursier output:"
            tail -20 "$cs_err" | sed 's/^/  /'
        fi
        rm -f "$cs_err"
    else
        FAILED["metals"]="coursier not available"
        echo "[metals] ✗ coursier not available"
        echo "        Install with: curl -fL https://github.com/coursier/launchers/raw/master/cs-aarch64-pc-linux.gz | gzip -d > /usr/local/bin/cs && chmod +x /usr/local/bin/cs"
    fi
fi

echo ""
echo "==============================================="
echo "Tier 5: Microsoft Ecosystem (2 servers)"
echo "==============================================="

# 14. C# - csharp-ls
download_dotnet "csharp-ls" "csharp-ls" "0.16.0"

# 15. F# - fsautocomplete
download_dotnet "fsautocomplete" "fsautocomplete" "0.83.0"

echo ""
echo "==============================================="
echo "Tier 6: Mobile, Systems & Other (5 servers)"
echo "==============================================="

# 16. Swift - sourcekit-lsp: not packaged. It comes with the Swift toolchain
# (registry.json says `system`), and Devscriptor runs the toolchain's own. A
# copy runs only beside that toolchain: the toolchain's binary loads its
# libraries from `../lib/swift/linux` and works with the compiler and SDK next
# to it, and swiftly's `bin/sourcekit-lsp` is swiftly itself, which runs the
# toolchain swiftly has set up. Packaged, it would run nowhere else, and stand
# in front of the user's toolchain. An earlier download's copy is removed so
# that it is not packaged.
rm -rf "${LSP_DIR:?}/sourcekit-lsp"
echo "[sourcekit-lsp] Not packaged: it comes with the Swift toolchain"

# 17. Dart - dart-ls (the official Dart SDK)
#
# The directory MUST be named `dart-ls`: the resolver looks under
# <install-root>/<server-id>/, and `dart-ls` is the id in registry.json.
#
# `dart-ls/` *is* a Dart SDK root, so `dart-ls/bin/dart language-server` runs
# without any Dart on the user's machine. Neither a wrapper around the build
# machine's `dart` nor a bare `analysis_server.dart.snapshot` would do: the
# wrapper needs that `dart`, and the snapshot only runs on the exact VM that
# built it and needs that VM's `lib/` to resolve `dart:core`.
#
# The SDK is fetched from Google's archive and its published SHA-256 is checked.
# The build is the one registry.json pins for dart-ls, which direct installs
# fetch too; set DART_SDK_VERSION to package another.
DART_SDK_VERSION="${DART_SDK_VERSION:-3.12.2}"
DART_SDK_PLATFORM="linux-arm64"
DART_ARCHIVE="https://storage.googleapis.com/dart-archive/channels/stable/release"

if [ -e "$LSP_DIR/dart-ls/bin/dart" ] && [ "$(cat "$LSP_DIR/dart-ls/.version" 2>/dev/null)" = "$DART_SDK_VERSION" ]; then
    already_present "dart-ls"
elif ! command_exists unzip; then
    FAILED["dart-ls"]="unzip is required"
    echo "[dart-ls] FAILED - install unzip"
else
    fetch_to() {  # fetch_to <url> <dest>; prints nothing, returns non-zero on failure
        if command_exists curl; then
            curl -fsSL -o "$2" "$1"
        elif command_exists wget; then
            wget -q -O "$2" "$1"
        else
            return 1
        fi
    }
    fetch_stdout() {
        if command_exists curl; then
            curl -fsSL "$1"
        elif command_exists wget; then
            wget -qO- "$1"
        else
            return 1
        fi
    }

    DART_VERSION="$DART_SDK_VERSION"
    DART_ZIP_URL="$DART_ARCHIVE/$DART_VERSION/sdk/dartsdk-$DART_SDK_PLATFORM-release.zip"
    DART_TMP=$(mktemp -d)
    echo "[dart-ls] Downloading Dart SDK $DART_VERSION ($DART_SDK_PLATFORM)"

    if ! fetch_to "$DART_ZIP_URL" "$DART_TMP/dartsdk.zip"; then
        FAILED["dart-ls"]="download failed"
        echo "[dart-ls] FAILED - could not download $DART_ZIP_URL"
        rm -rf "$DART_TMP"
    else
        # Google publishes a SHA-256 next to every archive. A truncated or
        # tampered 200 MB download must never be unpacked and shipped, nor one
        # whose published checksum could not be fetched.
        DART_SUM_EXPECTED=$(fetch_stdout "$DART_ZIP_URL.sha256sum" | awk '{print $1}' || true)
        DART_SUM_ACTUAL=$(sha256_of "$DART_TMP/dartsdk.zip")

        if [ -z "$DART_SUM_EXPECTED" ] || [ "$DART_SUM_EXPECTED" != "$DART_SUM_ACTUAL" ]; then
            FAILED["dart-ls"]="checksum mismatch"
            echo "[dart-ls] FAILED - SHA-256 mismatch (expected ${DART_SUM_EXPECTED:-none fetched}, got $DART_SUM_ACTUAL)"
            rm -rf "$DART_TMP"
        elif ! unzip -q "$DART_TMP/dartsdk.zip" -d "$DART_TMP"; then
            FAILED["dart-ls"]="extract failed"
            echo "[dart-ls] FAILED - could not unzip the Dart SDK"
            rm -rf "$DART_TMP"
        else
            # The archive holds a single `dart-sdk/` root; `dart-ls/` becomes it.
            rm -rf "$LSP_DIR/dart-ls"
            mkdir -p "$LSP_DIR"
            mv "$DART_TMP/dart-sdk" "$LSP_DIR/dart-ls"
            chmod +x "$LSP_DIR/dart-ls/bin/dart" 2>/dev/null || true
            printf '%s\n' "$DART_VERSION" > "$LSP_DIR/dart-ls/.version"
            rm -rf "$DART_TMP"
            DOWNLOADED["dart-ls"]="$DART_VERSION"
            echo "[dart-ls] OK (self-contained Dart SDK $DART_VERSION)"
        fi
    fi
fi

# 18. Ruby - ruby-lsp
# `ruby-lsp-launcher` is the binstub the registry points at; the plain
# `ruby-lsp` binstub shells out to bundler and needs a Gemfile. The version is
# the one registry.json pins; a bundle of another version is replaced, as is one
# built for another Ruby than this machine's. Its native extensions are built
# for the Ruby that installs it, and Devscriptor installs it again, with the
# user's Ruby, wherever that is another one.
RUBY_LSP_VERSION="0.26.11"
if [ -e "$LSP_DIR/ruby-lsp/bin/ruby-lsp-launcher" ] && [ "$(cat "$LSP_DIR/ruby-lsp/.version" 2>/dev/null)" = "$RUBY_LSP_VERSION" ] &&
   built_for_this_ruby "$LSP_DIR/ruby-lsp"; then
    already_present "ruby-lsp"
else
    download_gem "ruby-lsp" "ruby-lsp" "$RUBY_LSP_VERSION"
    if [ -n "${DOWNLOADED[ruby-lsp]:-}" ]; then
        printf '%s\n' "$RUBY_LSP_VERSION" > "$LSP_DIR/ruby-lsp/.version"
    fi
fi

# 19. PHP - phpactor
if [ -e "$LSP_DIR/phpactor/bin/phpactor" ]; then
    already_present "phpactor"
else
    echo "[phpactor] Downloading the latest release's phpactor.phar"
    mkdir -p "$LSP_DIR/phpactor/bin"
    if fetch_release_asset phpactor/phpactor '^phpactor\.phar$' "$LSP_DIR/phpactor/bin/phpactor" >/dev/null; then
        chmod +x "$LSP_DIR/phpactor/bin/phpactor"
        DOWNLOADED["phpactor"]="github:phpactor/phpactor"
        echo "[phpactor] ✓ Downloaded successfully"
    else
        rm -rf "${LSP_DIR:?}/phpactor"
        FAILED["phpactor"]="release asset not fetched"
        echo "[phpactor] ✗ Could not fetch a verified phpactor.phar"
    fi
fi

# 20. Bash - bash-language-server
download_npm "bash-language-server" "bash-language-server" "5.6.0"

# 21. Solidity - nomicfoundation-solc
download_npm "nomicfoundation-solc" "@nomicfoundation/solidity-language-server" "0.8.25"

echo ""
echo "==============================================="
echo "Download Summary"
echo "==============================================="
echo ""
echo "Successfully downloaded: ${#DOWNLOADED[@]}"
for name in "${!DOWNLOADED[@]}"; do
    echo "  ✓ $name (${DOWNLOADED[$name]})"
done

echo ""
echo "Failed/Skipped: ${#FAILED[@]}"
for name in "${!FAILED[@]}"; do
    echo "  ✗ $name - ${FAILED[$name]}"
done

echo ""
echo "Total size:"
du -sh "$LSP_DIR" 2>/dev/null || echo "  (unable to calculate)"

echo ""
echo "==============================================="
echo "LSP Server download complete"
echo "==============================================="
