# Language server prerequisites

What each of the 21 language servers Devscriptor supports needs in order to run.
The servers are not part of the Devscriptor binary: they are found on your system
or in editor extensions, or installed on request by `devscriptor lsp setup` (see
[Language servers](lsp.md#installing-servers)). `devscriptor lsp check <server>`
reports what a missing server lacks.

- [At a glance](#at-a-glance)
- [Self-contained servers](#self-contained-servers)
- [Node.js servers](#nodejs-servers)
- [.NET servers](#net-servers)
- [Java servers](#java-servers)
- [PHP and Ruby](#php-and-ruby)
- [Swift](#swift)
- [Troubleshooting](#troubleshooting)

Sizes are those of a Linux x86_64 install.

## At a glance

| Server | Languages | Installed from | Version | Needs to run |
| --- | --- | --- | --- | --- |
| `bash-language-server` | Bash | npm | 5.6.0 | Node.js 18+ |
| `clangd` | C, C++, CUDA, Objective-C, Objective-C++ | GitHub release | latest | Nothing |
| `csharp-ls` | C# | `dotnet tool` | 0.16.0 | .NET SDK |
| `dart-ls` | Dart | Google's Dart SDK archive | 3.12.2 | Nothing (it is the Dart SDK) |
| `fsautocomplete` | F# | `dotnet tool` | 0.83.0 | .NET SDK |
| `gopls` | Go | `go install` | latest | Go 1.20+ |
| `graphql-language-service` | GraphQL | npm (`graphql-language-service-cli`) | 3.5.0 | Node.js 18+ |
| `html-languageserver` | HTML | npm (`vscode-langservers-extracted`) | 4.10.0 | Node.js 18+ |
| `jdtls` | Java | Eclipse milestone archive | 1.61.0 | Java 21+ and Python 3 |
| `kotlin-lsp` | Kotlin | JetBrains' archive | latest | Nothing (it bundles its Java runtime) |
| `metals` | Scala | Coursier | 1.6.7 | Java |
| `nomicfoundation-solc` | Solidity | npm (`@nomicfoundation/solidity-language-server`) | 0.8.25 | Node.js 18+ |
| `phpactor` | PHP | GitHub release | latest | PHP 8.2+ with mbstring, posix, tokenizer; not on Windows |
| `pyright` | Python | npm | 1.1.411 | Node.js 18+ |
| `ruby-lsp` | Ruby | gem | 0.26.11 | Ruby 3.0+ |
| `rust-analyzer` | Rust | GitHub release | latest | rustup, to load a project |
| `sourcekit-lsp` | Swift, Objective-C | The Swift toolchain | — | The Swift toolchain |
| `sql-language-server` | SQL | npm | 1.7.1 | Node.js 18+ |
| `svelte-language-server` | Svelte | npm | 0.18.3 | Node.js 18+ |
| `typescript-language-server` | TypeScript, JavaScript, TSX | npm | 5.3.0 | Node.js 18+ |
| `volar` | Vue | npm (`@vue/language-server`) | 2.2.12 | Node.js 18+ |

The versions are those `crates/lsp/src/manager/registry.json` pins; "latest" is
the newest release at install time.

## Self-contained servers

### clangd (C, C++, CUDA, Objective-C)

- **Size:** about 160 MB: `bin/clangd` and the builtin headers.
- **Source:** clangd's own GitHub release (`clangd-linux-<version>.zip`,
  `clangd-mac-…`, `clangd-windows-…`), checked against the SHA-256 GitHub
  publishes for it.
- **Platforms:** Linux x86_64, macOS (a universal binary) and Windows. clangd
  publishes no Linux ARM build, and a distribution's clangd links the system's
  libclang-cpp and libLLVM, so on Linux aarch64 Devscriptor runs the system's
  (`apt install clangd`), and `lsp setup` refuses to install the x86_64 build
  there.
- **The `clangd` on the PATH is never copied into a package:** it may be a
  toolchain manager's proxy (swiftly puts one on the PATH under that name) or, on
  macOS, `/usr/bin/clangd`, a shim into the Command Line Tools.
- **Projects without a compilation database** (`compile_commands.json` at the
  root or in `build/`, `compile_flags.txt`, `.clangd`) get one that Devscriptor
  writes under `<cache>/devscriptor/clangd/`, never into the project: each C, C++
  and CUDA file with its language (a `.cuh` header as CUDA) and the project's
  `include/` directories, and, where no CUDA installation is found (`CUDA_PATH`,
  `CUDA_HOME`, `/usr/local/cuda`), the flags that let clang parse CUDA without
  one. A project inside the workspace that has its own compilation database keeps
  it: its `compile_commands.json` (or `build/`'s) gives its files' commands as
  written, and its `compile_flags.txt` their flags. A written database whose files
  are all gone is removed when the next one is written.

### rust-analyzer (Rust)

- **Size:** about 40 MB.
- **Source:** the standalone release asset from GitHub
  (`rust-analyzer-<arch>-unknown-linux-gnu.gz` and its macOS and Windows
  counterparts). Neither rustup's `~/.cargo/bin/rust-analyzer` proxy nor the
  toolchain's own copy is packaged: the proxy fails outside rustup, and the
  toolchain copy links `librustc_driver-*.so`.
- **Needs:** nothing to start, but rust-analyzer loads a project through `cargo`,
  so a Rust toolchain must be installed for it to analyse anything.
- **Builds** in `target/rust-analyzer`, off the project's own build directory
  lock (see [Language servers](lsp.md#lifecycle)).

### dart-ls (Dart)

- **Size:** about 620 MB, a complete Dart SDK, started as `bin/dart language-server`.
- **Source:** the Dart SDK release archive from `storage.googleapis.com/dart-archive`,
  at the version the registry pins, checked against the SHA-256 Google publishes
  for each platform's archive.
- **Needs:** nothing: no Dart or Flutter installation on the machine.

### kotlin-lsp (Kotlin)

- **Size:** about 510 MB unpacked, from a ~350 MB download.
- **Binary:** `bin/intellij-server`, started as
  `intellij-server --stdio --system-path=<cache>/devscriptor/kotlin-lsp`, where it
  keeps its caches and indexes.
- **Source:** JetBrains' standalone Kotlin LSP archive for the platform, at its
  latest build (that of the latest release of
  [Kotlin/kotlin-lsp](https://github.com/Kotlin/kotlin-lsp)), checked against the
  SHA-256 JetBrains publishes beside it. It is not on the Devscriptor mirror.
- **Needs:** nothing: the archive bundles the JetBrains Runtime it runs on.
- **Updates:** a build stops starting some weeks after its release, saying "This
  build of kotlin-server has expired"; Devscriptor's error then names the command
  that installs the current build, `devscriptor lsp setup kotlin`, which replaces
  an install of an earlier one.
- **Why this server:** kotlin-language-server is deprecated and does not start on
  Java 25 or newer.

### gopls (Go)

- **Size:** about 40 MB.
- **Source:** `go install golang.org/x/tools/gopls@latest`.
- **Needs:** Go 1.20 or newer, to install it and to load a project's packages.

## Node.js servers

`bash-language-server`, `graphql-language-service`, `html-languageserver`,
`nomicfoundation-solc`, `pyright`, `sql-language-server`,
`svelte-language-server`, `typescript-language-server` and `volar` are npm
packages, installed with their dependencies in their own `node_modules/`. They need
**Node.js 18 or newer** to run (and npm to install).

| Server | Size | Binary |
| --- | --- | --- |
| `bash-language-server` | 14 MB | `node_modules/.bin/bash-language-server` |
| `graphql-language-service` | 95 MB | `node_modules/.bin/graphql-lsp` |
| `html-languageserver` | 78 MB | `node_modules/.bin/vscode-html-language-server` |
| `nomicfoundation-solc` | 46 MB | `node_modules/.bin/nomicfoundation-solidity-language-server` |
| `pyright` | 19 MB | `node_modules/.bin/pyright-langserver` |
| `sql-language-server` | 72 MB | `node_modules/.bin/sql-language-server` |
| `svelte-language-server` | 50 MB | `node_modules/.bin/svelteserver` |
| `typescript-language-server` | 25 MB | `node_modules/.bin/typescript-language-server` |
| `volar` | 47 MB | `node_modules/.bin/vue-language-server` |

## .NET servers

`csharp-ls` (about 110 MB) and `fsautocomplete` (about 340 MB) are .NET tools,
installed with `dotnet tool install` at the versions the registry pins. They need
the **.NET SDK** (`dotnet` on the PATH, or `DOTNET_ROOT`). Both run with
`DOTNET_ROLL_FORWARD=Major`, so a newer .NET than the one they were built for runs
them, and a .NET tool is told where `dotnet` is.

## Java servers

### jdtls (Java)

- **Size:** about 65 MB.
- **Source:** Eclipse's milestone archive at the version the registry pins, checked
  against the SHA-256 Eclipse publishes beside it.
- **Needs:** **Java 21 or newer** and **Python 3**: it starts through `bin/jdtls`,
  a Python launcher that runs Java (`JAVA_HOME`'s when set, else `java` on the
  PATH) and refuses an older Java.

### metals (Scala)

- **Size:** about 130 MB: a launcher holding Metals and every library it runs on.
- **Binary:** `bin/metals` (`bin/metals.bat` on Windows), run by `java`.
- **Source:** Coursier, `cs bootstrap --standalone`, so Metals starts without the
  network. Coursier's default thin launcher would fetch the libraries into its
  cache at the first start; `devscriptor lsp setup metals` and the download
  scripts replace a thin launcher they find.
- **Needs:** **Java**, and the project's build tool on `PATH` (`sbt` for an sbt
  build) to import the build, which Metals does without asking. Without it, Metals
  answers hover, definitions and references, but not workspace symbols or semantic
  tokens. Metals still fetches what a project's build needs, as the build would:
  the presentation compiler for another Scala version, Bloop.

## PHP and Ruby

### phpactor (PHP)

- **Size:** about 4 MB, a PHAR archive at `bin/phpactor`, from phpactor's GitHub
  releases.
- **Needs:** **PHP 8.2 or newer**, `php` on the PATH, with the **mbstring**,
  **posix** and **tokenizer** extensions, without which phpactor does not start.
  Debian's and Ubuntu's `php-cli` lacks mbstring (`apt install php-mbstring`; the
  Linux `install-dependencies.sh` installs it). PHP on Windows has no posix
  extension, so phpactor is not installed on Windows.

### ruby-lsp (Ruby)

- **Size:** about 30 MB.
- **Binary:** `bin/ruby-lsp-launcher`, run with the `ruby` on the PATH rather than
  its shebang, which names the Ruby the package was built with.
- **Source:** the mirror's package, or `gem install ruby-lsp` at the version the
  registry pins, with this machine's Ruby.
- **Needs:** **Ruby 3.0 or newer**. Its gems' native extensions (`prism`, `rbs`)
  load only on the Ruby API version they were built for: a package built for
  another Ruby is skipped, with a warning naming both versions, and
  `devscriptor lsp setup ruby-lsp` installs it again with this machine's Ruby,
  which needs that Ruby's headers and a C compiler
  (`apt install ruby-dev build-essential`).

## Swift

### sourcekit-lsp (Swift, Objective-C)

sourcekit-lsp runs only inside its Swift toolchain: the binary loads its libraries
from `../lib/swift/<platform>` and works with the compiler and SDK beside it, and
swiftly's `bin/sourcekit-lsp` is swiftly itself, which runs the toolchain swiftly
has set up. So it is never packaged: the download scripts leave it out, and
`devscriptor lsp setup` does not install it. Devscriptor runs the one in the
toolchain (on the PATH, in `/usr/bin`, or in swiftly's `~/.local/share/swiftly/bin`),
and skips a copy in the managed directory. Install a toolchain from
[swift.org](https://swift.org/download/).

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `cannot execute binary file: Exec format error` | A build for another architecture | Use the download script, or `lsp setup`, of your platform |
| `shared library not found` | A missing system library or runtime | Install the runtime the server needs (above) |
| An npm server is `not found` or does not start | Node.js missing, or older than 18 | Install Node.js 18 or newer, with `node` and `npm` on the PATH |
| rust-analyzer starts but finds nothing | No Rust toolchain: it loads a project through `cargo` | Install Rust with rustup |
| ruby-lsp is skipped with a version warning | Its package was built for another Ruby | `devscriptor lsp setup ruby-lsp` |
