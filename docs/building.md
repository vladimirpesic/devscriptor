# Building and releasing

- [Building from source](#building-from-source)
- [Features](#features)
- [Tests](#tests)
- [Release builds](#release-builds)
- [The language server mirror](#the-language-server-mirror)
  - [Publishing the mirror](#publishing-the-mirror)
  - [Testing packages locally](#testing-packages-locally)
- [The embedding model mirror](#the-embedding-model-mirror)
- [Troubleshooting](#troubleshooting)

## Building from source

Devscriptor needs Rust 1.95 or newer (edition 2024) and a C compiler, for the
tree-sitter grammars and the bundled SQLite: `build-essential` on Linux, the Xcode
Command Line Tools on macOS, the Visual Studio C++ Build Tools on Windows. TLS is
rustls, so no OpenSSL headers are needed.

```bash
cargo build --release -p devscriptor-cli       # target/release/devscriptor
```

`scripts/<platform>/install-dependencies.{sh,ps1}` install the C toolchain together
with the language runtimes the language servers need (Node.js, Java, Go, .NET, …),
which matter only for running or packaging those servers.

The repository's `.cargo/config.toml` limits a build to two parallel jobs and the
tests to one thread per test binary, so that the build and the test suite fit a
4-core, 8 GB machine; pass `-j N` to build faster where there is the memory.

## Features

`devscriptor-cli`'s default features build the full binary: every language
(`full-lang`), the embedding model download (`model-download`), the
language-server integration (`lsp`) and the MCP server (`mcp`), with all 75 MCP
tools. Release builds use the defaults.

| Build | MCP tools |
| --- | ---: |
| `cargo build -p devscriptor-cli` (the defaults) | 75 |
| `cargo build -p devscriptor-mcp` — the development binary `devscriptor-mcp-standalone`, without the `lsp` feature | 32 |
| `cargo build -p devscriptor-mcp --features lsp` | 75 |

The core library builds with a subset of the languages through its `lang-*`
features (`lang-rust`, `lang-python`, …) instead of `full-lang`, and without
`model-download` the model must be placed in its cache by hand (see
[Privacy](privacy.md#air-gapped-and-offline-use)).

## Tests

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The workspace's tests embed with `MockEmbeddingProvider`, which needs no model.
The tests of the scripts run the Linux ones on Linux only; on macOS, the macOS
downloader's need Homebrew's bash (`brew install bash`), as the script itself
does, and each macOS `install-dependencies.sh` installs it.
They include the golden corpus — each grammar's entity counts on the fixtures in
`crates/core/tests/test_files/`, within a tolerance of the sidecars in
`crates/core/tests/golden/` — and a labeled precision/recall fixture for call edges
(`crates/core/tests/labeled/`). The tests of the shared per-workspace server
(`crates/cli/tests/serve.rs`, Unix only) run the binary as server and proxies,
each with its own runtime, data and configuration directories, and stop every
server they started.

**Slow tests** use the real embedding model (downloaded on first use) and are
gated by the `slow-tests` feature:

```bash
cargo test -p devscriptor-core --features slow-tests --release
cargo test -p devscriptor-mcp --features slow-tests --release
```

**Ignored tests** need the network or change checked-in files:

```bash
# Regenerate the golden sidecars after an intentional extraction change
cargo test -p devscriptor-core --test golden_corpus -- --ignored generate_golden_sidecars

# Install the npm, Coursier, gem and Go servers; install the pinned downloads from
# their publishers (~650 MB); check the pinned checksums and license files against
# the publishers'
cargo test -p devscriptor-lsp --lib -- --ignored package_manager_installs_start
cargo test -p devscriptor-lsp --lib -- --ignored pinned_downloads_install_from_their_publishers
cargo test -p devscriptor-lsp --lib -- --ignored pinned_checksums_match_the_publishers
cargo test -p devscriptor-lsp --lib -- --ignored pinned_license_files_match_the_publishers
```

**The VS Code extension**:

```bash
cd vscode-extension
npm install
npm run lint
npm run test:unit      # parsers, the MCP client against a stand-in server
DEVSCRIPTOR_BIN=../target/debug/devscriptor npm run test:unit   # also: sharing one Devscriptor process
npm test               # every test, in a VS Code it downloads (needs a display; Xvfb on CI)
```

CI (`.github/workflows/ci.yml`) runs on every push to `main` and every pull request:
rustfmt; clippy and the tests on each supported platform — Linux x64 and ARM64,
macOS on Apple Silicon and Intel, Windows x64; the extension's lint and unit tests;
and `cargo audit`. A failed clippy or test step is reported as annotations: each
compiler error, each failed test's output and the failed targets
(`.github/scripts/annotate-failures.py`). The nightly workflow
(`.github/workflows/nightly.yml`) runs the slow tests with the real model and every
extension test in VS Code.

## Release builds

The release workflow (`.github/workflows/release.yml`) builds every archive on
GitHub Actions, each on its own platform's runner, and checks each binary: it
reports the version in `Cargo.toml`, its MCP server starts with all 75 tools, it
needs glibc 2.17 at most on Linux and macOS 11 on macOS, and it links no C++
runtime or OpenSSL, nor on Windows the Visual C++ runtime. On Windows the binary
also reserves the 8 MiB main-thread stack that Linux and macOS give a program
(`crates/cli/build.rs`); Windows' own 1 MiB is too little for a debug build. Pushing a tag
`v<version>` builds the archives and publishes them, with their checksums and
`latest.json`, as a GitHub release; the tag must match the version in
`Cargo.toml`, and the release waits for a check that the mirror releases the
commit pins are published, whole (`.github/scripts/check-mirrors.py`). A manual run, or a push to `main` that changes the workflow, builds
and checks them without publishing them. The sections below build the same
archives by hand.

A release is one archive per platform, each holding a directory
`devscriptor-<version>-<target>/` with exactly one file, `devscriptor`
(`devscriptor.exe` on Windows), and a `.sha256` beside it naming the archive
without a path. Devscriptor links no C++ runtime and no system library besides
libc, so the only portability concern on Linux is the **glibc floor**, pinned with
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild).

| Platform | Target | Build machine | Tool | Archive |
| --- | --- | --- | --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` | x86_64 Linux | `cargo zigbuild` (glibc 2.17) | `.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | ARM64 Linux, or x86_64 Linux (cross) | `cargo zigbuild` (glibc 2.17) | `.tar.gz` |
| macOS Apple Silicon | `aarch64-apple-darwin` | an Apple Silicon Mac | `cargo build` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | an Apple Silicon Mac (cross) or an Intel Mac | `cargo build` | `.tar.gz` |
| Windows x64 | `x86_64-pc-windows-msvc` | Windows with the MSVC build tools | `cargo build` | `.zip` |

Before building on any machine:

1. The version in `Cargo.toml` (`[workspace.package] version`) and `Cargo.lock` is
   the one being released, committed, and checked out on every build machine;
   otherwise `--locked` fails or the binary reports the wrong version.
2. Rust is a recent stable (`rustup update stable`).
3. No `--features`: the defaults are the release build.

Each bash block below runs as one subshell `( … )` from the repository root:
`set -e` pasted into an interactive shell would close the shell on the first
failure.

### Linux

A plain `cargo build` links against the build machine's glibc, so a binary built on
a recent distribution refuses to start on older ones
(`version 'GLIBC_2.xx' not found`). `cargo zigbuild --target <triple>.2.17` pins the
floor to glibc 2.17, and cross-compiles aarch64 from an x86_64 host. Install it once,
with Zig 0.13 or newer on the `PATH`:

```bash
cargo install cargo-zigbuild
```

```bash
(
set -e
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
for TARGET in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  NAME="devscriptor-${VERSION}-${TARGET}"
  rustup target add "${TARGET}"
  cargo zigbuild --release --locked --target "${TARGET}.2.17" -p devscriptor-cli

  rm -rf "dist/${NAME}" && mkdir -p "dist/${NAME}"
  cp "target/${TARGET}/release/devscriptor" "dist/${NAME}/devscriptor"
  rm -f "dist/${NAME}.tar.gz"
  tar -czf "dist/${NAME}.tar.gz" -C dist "${NAME}"
  ( cd dist && sha256sum "${NAME}.tar.gz" > "${NAME}.tar.gz.sha256" )
  rm -rf "dist/${NAME}"

  # The glibc floor must be <= 2.17.
  objdump -T "target/${TARGET}/release/devscriptor" | grep -oE 'GLIBC_[0-9.]+' | sort -V | tail -1
done

./target/x86_64-unknown-linux-gnu/release/devscriptor --version
ldd target/x86_64-unknown-linux-gnu/release/devscriptor | grep -cE 'stdc\+\+|ssl' || true   # 0
file target/aarch64-unknown-linux-gnu/release/devscriptor                                   # ARM aarch64
)
```

### macOS

On an Apple Silicon Mac with the Xcode Command Line Tools
(`xcode-select --install`), which builds `aarch64-apple-darwin` natively and
cross-builds `x86_64-apple-darwin` with no extra toolchain:

```bash
(
set -e
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
export MACOSX_DEPLOYMENT_TARGET=11.0   # the oldest macOS supported
for TARGET in aarch64-apple-darwin x86_64-apple-darwin; do
  NAME="devscriptor-${VERSION}-${TARGET}"
  rustup target add "${TARGET}"
  cargo build --release --locked --target "${TARGET}" -p devscriptor-cli

  rm -rf "dist/${NAME}" && mkdir -p "dist/${NAME}"
  cp "target/${TARGET}/release/devscriptor" "dist/${NAME}/devscriptor"
  rm -f "dist/${NAME}.tar.gz"
  COPYFILE_DISABLE=1 tar -czf "dist/${NAME}.tar.gz" -C dist "${NAME}"   # no ._AppleDouble files
  ( cd dist && shasum -a 256 "${NAME}.tar.gz" > "${NAME}.tar.gz.sha256" )
  rm -rf "dist/${NAME}"
done

./target/aarch64-apple-darwin/release/devscriptor --version
file target/x86_64-apple-darwin/release/devscriptor          # Mach-O 64-bit executable x86_64
)
```

The binaries are unsigned: a first launch may need
`xattr -d com.apple.quarantine …`. With an Apple Developer ID, sign and notarize
the binary before packaging it.

### Windows

In PowerShell, with the Visual Studio C++ Build Tools (the MSVC `link.exe`) and
rustup:

```powershell
$ErrorActionPreference = "Stop"
$VERSION = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
$TARGET = "x86_64-pc-windows-msvc"; $NAME = "devscriptor-$VERSION-$TARGET"

rustup target add $TARGET
$env:RUSTFLAGS = "-C target-feature=+crt-static"   # no Visual C++ redistributable needed
cargo build --release --locked --target $TARGET -p devscriptor-cli

Remove-Item -Recurse -Force "dist\$NAME" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "dist\$NAME" | Out-Null
Copy-Item "target\$TARGET\release\devscriptor.exe" "dist\$NAME\devscriptor.exe"
Remove-Item -Force "dist\$NAME.zip" -ErrorAction SilentlyContinue
Compress-Archive -Path "dist\$NAME" -DestinationPath "dist\$NAME.zip"
(Get-FileHash -Algorithm SHA256 "dist\$NAME.zip").Hash.ToLower() + "  $NAME.zip" |
    Set-Content "dist\$NAME.zip.sha256"
Remove-Item -Recurse -Force "dist\$NAME"

& "target\$TARGET\release\devscriptor.exe" --version
```

### After the builds

On a machine where the binary runs, check that the MCP server starts with every
tool:

```bash
devscriptor --version
devscriptor --verbose mcp --no-daemon </dev/null 2>&1 >/dev/null | grep -m1 "Starting Unified MCP Server"
# Starting Unified MCP Server (75 tools)
```

`dist/` then holds the five archives, each with its `.sha256`:

```text
devscriptor-<version>-x86_64-unknown-linux-gnu.tar.gz
devscriptor-<version>-aarch64-unknown-linux-gnu.tar.gz
devscriptor-<version>-aarch64-apple-darwin.tar.gz
devscriptor-<version>-x86_64-apple-darwin.tar.gz
devscriptor-<version>-x86_64-pc-windows-msvc.zip
```

The release workflow publishes them with their checksums and a `latest.json` that
names the release. The VS Code extension's *Check for Devscriptor Updates* reads
the `tag_name` of the latest release's `latest.json`,
`https://github.com/vladimirpesic/devscriptor/releases/latest/download/latest.json`:

```json
{
  "tag_name": "v1.0.0",
  "published_at": "2026-01-01T00:00:00Z",
  "assets": [
    { "name": "devscriptor-1.0.0-x86_64-unknown-linux-gnu.tar.gz", "platform": "linux-x64" },
    { "name": "devscriptor-1.0.0-aarch64-unknown-linux-gnu.tar.gz", "platform": "linux-arm64" },
    { "name": "devscriptor-1.0.0-aarch64-apple-darwin.tar.gz", "platform": "macos-arm64" },
    { "name": "devscriptor-1.0.0-x86_64-apple-darwin.tar.gz", "platform": "macos-x64" },
    { "name": "devscriptor-1.0.0-x86_64-pc-windows-msvc.zip", "platform": "windows-x64" }
  ]
}
```

Tag the release commit `v<version>`; pushing the tag runs the release workflow.
Versions follow
[Semantic Versioning](https://semver.org/); the tag carries a `v`, the version in
`Cargo.toml` and the archive names do not.

## The language server mirror

`devscriptor lsp setup` installs a server from Devscriptor's mirror first, and from
the server's own source (its GitHub release, npm, Go, …) when the mirror has none
for the platform. The mirror is a GitHub release, `lsp-servers-<date>-<commit>`, that holds
every platform's archives side by side, with each platform's
`manifest-<platform>.json` and `sha256sums-<platform>.txt`.
`crates/lsp/src/manager/mirror.json` pins the release: its tag, and each
platform's manifest, which names every server's archive and its SHA-256. The pin
is compiled in, so a build installs only the archives it was built with, each
checked before it is unpacked; an archive without the server's binary is refused,
and the server is installed from its own source instead.
`DEVSCRIPTOR_LSP_MIRROR_URL` fetches the archives from another place that holds
them side by side, such as a copy of the release.

Every archive carries its server's license files, which `license_files` in
`crates/lsp/src/manager/registry.json` names: a file the server ships (an npm
package's `LICENSE`, the Dart SDK's) is copied, and one it does not (a bare
binary such as rust-analyzer's) is fetched from a commit of its publisher's
repository and checked against the SHA-256 the registry pins. A server whose
license files cannot be had is not packaged.

The platforms are `linux-x86_64`, `linux-aarch64`, `macos-x86_64`,
`macos-aarch64` and `windows-x86_64`, and each has a directory in `scripts/`:

| Script | What it does |
| --- | --- |
| `install-dependencies.{sh,ps1}` | Installs the toolchains the downloads use (Node.js, Go, .NET, Ruby, Coursier, PHP and more): as root on Linux, as the user with Homebrew on macOS, with winget on Windows |
| `download-lsp-servers.{sh,ps1}` | Fetches the 20 packaged servers (all but sourcekit-lsp) into `lsp-servers/`, at the versions the registry pins, release downloads checked against their published or pinned SHA-256; one that cannot be fetched is reported and the others go on |
| `package-lsp-servers.{sh,ps1}` | Packs each server in `lsp-servers/` into `lsp-packages/<id>-<version>-<platform>.tar.gz` (`.zip` on Windows) with its license files under `LICENSES/`, and writes `sha256sums-<platform>.txt` and `manifest-<platform>.json` |

### Publishing the mirror

The LSP mirror workflow (`.github/workflows/lsp-mirror.yml`), run by hand from the
Actions tab, publishes a release of the mirror from GitHub's runners:

1. It drafts the release `lsp-servers-<date>-<commit>`, named after the day and
   the commit it packages from.
2. On each platform's runner, it sets up the toolchains the downloads use, runs
   the platform's scripts (all but the Windows `install-dependencies.ps1`, which
   works through winget, which the runner has not), pins the new manifest,
   checks that it holds every server the registry installs on the platform,
   serves the packages from a local web server, and installs each from there
   with the downloader's own code (`crates/lsp/tests/mirror_packages.rs`). Then
   it uploads them to the draft.
3. Once every platform has, it pins the five manifests together and runs the
   mirror's tests: every server the registry installs on a platform is packaged
   for it, at the build the registry pins. Only then does it publish the release,
   which never becomes the latest release: that stays Devscriptor's.

A platform that fails leaves the draft unpublished, and re-running its failed job
uploads to the same draft. A re-run uses the commit its run started from, so a
fix to the scripts needs a new run of the workflow, which replaces the draft.
Once the release is published, pin it and commit the pin:

```bash
scripts/pin-lsp-mirror.py lsp-servers-<date>-<commit>
cargo test -p devscriptor-lsp --lib manager::mirror
git add crates/lsp/src/manager/mirror.json
```

A published release never changes: other servers make a new release, and an older
one stays for the builds that pin it. Never delete one: GitHub retires the tag of a
deleted immutable release for good, so the builds that pin it could not have it
back. The release workflow publishes no Devscriptor release before the mirror
release it pins.

### Testing packages locally

A platform's packages can be installed the way the workflow installs them, before
anything is published:

```bash
( set -e
  cd scripts/linux-x86_64 && ./download-lsp-servers.sh && ./package-lsp-servers.sh && cd ../..
  scripts/pin-lsp-mirror.py lsp-servers-local scripts/linux-x86_64/lsp-packages/manifest-linux-x86_64.json
  python3 -m http.server 8765 --bind 127.0.0.1 --directory scripts/linux-x86_64/lsp-packages &
  server=$!
  DEVSCRIPTOR_LSP_MIRROR_URL=http://127.0.0.1:8765 \
    cargo test -p devscriptor-lsp --test mirror_packages -- --ignored --nocapture || true
  kill "$server"
  git checkout crates/lsp/src/manager/mirror.json )
```

## The embedding model mirror

The model is downloaded from Hugging Face, at the commit Devscriptor pins
(`PINNED_HF_REVISION` in `crates/core/src/embeddings/model_fetch.rs`), with the
mirror as the fallback: the GitHub release
`jina-embeddings-v2-base-code-<commit>` (`MODEL_MIRROR_RELEASE`), which holds
the same files side by side (`model_quantized.onnx` beside `tokenizer.json`). Each
file is checked against the SHA-256 Devscriptor pins, whichever source it comes
from. The model mirror workflow (`.github/workflows/model-mirror.yml`), run by hand
once for each pinned commit, publishes the release: it fetches the files with
`scripts/download-embedding-model.sh` and publishes them with a `sha256sums.txt`.
The release never becomes the latest release, and never changes. It is never
deleted either: its tag would be retired, and a new release would need a new
`MODEL_MIRROR_RELEASE`.

The same script provisions the model cache ahead of time, at that commit and
checked against the pinned SHA-256 hashes: into the cache by default, or into
`$DEVSCRIPTOR_MODEL_DIR`:

```text
jina-embeddings-v2-base-code/
├── onnx/model_quantized.onnx
├── tokenizer.json
├── config.json
├── tokenizer_config.json
└── special_tokens_map.json
```

## Troubleshooting

| Problem | Cause and fix |
| --- | --- |
| `version 'GLIBC_2.xx' not found` on another machine | Built with a plain `cargo build` on a newer distribution: build with `cargo zigbuild --target <triple>.2.17`, and check the floor with `objdump -T` |
| `openssl-sys` appears in a build | A dependency enabled `native-tls`: `cargo tree -i openssl-sys` finds it, and disabling its default features removes it |
| A linker error for a target | The target is not installed: `rustup target add <triple>` |
| `--locked` fails | `Cargo.lock` does not match `Cargo.toml`: commit both after changing the version |
| The aarch64 binary crashes with `SIGILL` | CPU-specific flags (an SVE baseline, say) passed through `CFLAGS` apply to every C dependency; build without them |
