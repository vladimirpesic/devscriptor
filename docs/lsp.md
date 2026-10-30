# Language servers

Devscriptor drives language servers for live, type-aware navigation: hover,
definitions, references, implementations, call and type hierarchies, symbols,
diagnostics, code actions, formatting and renames. The MCP server offers them as
38 LSP tools and 5 architecture tools (see [MCP server](mcp.md#lsp-tools-38)); the
CLI has `devscriptor lsp`, `definition` and `references`. This page covers which
servers there are, how Devscriptor finds, installs, starts and stops them, and how
to configure them.

- [Supported servers](#supported-servers)
- [Finding a server](#finding-a-server)
- [Installing servers](#installing-servers)
- [Lifecycle](#lifecycle)
- [Reliability](#reliability)
- [What servers write into a project](#what-servers-write-into-a-project)
- [Positions](#positions)
- [Configuration file](#configuration-file)
- [CLI commands](#cli-commands)
- [Troubleshooting](#troubleshooting)

## Supported servers

| Server | Languages |
| --- | --- |
| rust-analyzer | Rust |
| clangd | C, C++, CUDA, Objective-C, Objective-C++ |
| gopls | Go |
| typescript-language-server | TypeScript, JavaScript, TSX |
| volar | Vue |
| svelte-language-server | Svelte |
| html-languageserver | HTML |
| graphql-language-service | GraphQL |
| pyright | Python |
| sql-language-server | SQL |
| jdtls | Java |
| kotlin-lsp | Kotlin |
| metals | Scala |
| csharp-ls | C# |
| fsautocomplete | F# |
| sourcekit-lsp | Swift |
| dart-ls | Dart |
| ruby-lsp | Ruby |
| bash-language-server | Bash |
| nomicfoundation-solc | Solidity |
| phpactor | PHP |

What each needs in order to run — Node.js, Java, .NET, a toolchain — is in
[Language server prerequisites](lsp-servers.md).

- **Objective-C.** MATLAB uses `.m` too, so a `.m` file goes to clangd only when
  its content is Objective-C (`@interface`, `@implementation` or `#import` in its
  first 4 KiB), as the scanner tells them apart; a MATLAB file is answered that no
  server reads it. Without the Apple SDK, as on Linux, clangd cannot resolve
  Foundation's and UIKit's types, but still answers for the project's own
  declarations.
- **VB.NET** files are indexed but have no language server: csharp-ls serves C#
  only.
- **GraphQL.** graphql-language-service serves a project only below a GraphQL
  config, by any name graphql-config accepts:
  `graphql.config.{ts,cts,mts,js,cjs,mjs,json,yaml,yml,toml}`, `.graphqlrc`,
  `.graphqlrc.{ts,cts,mts,js,cjs,mjs,json,yml,yaml,toml}`, `.graphqlconfig` and
  `.graphqlconfig.{json,yaml,yml}`. (A `graphql` key in `package.json` also
  configures it, but is not detected.)

## Finding a server

Devscriptor looks for a server in four places, in order, and uses the first it
finds:

1. **A configuration file.** The file `DEVSCRIPTOR_LSP_CONFIG` names, or a
   `devscriptor-lsp.toml` in the working directory that you have trusted (see
   [Configuration file](#configuration-file)). When one applies it is
   authoritative: the LSP tools use exactly the servers it lists.
2. **Devscriptor's own installs**: `<cache>/devscriptor/lsp-servers/<id>/` (where
   `lsp setup` installs; `<cache>` is `~/.cache` on Linux, `~/Library/Caches` on
   macOS, `%LOCALAPPDATA%` on Windows), then a `bundle/lsp-servers/` directory
   beside the executable (or one or two directories above it, or in the working
   directory). An install of jdtls, kotlin-lsp or the Dart SDK records its version;
   one at another version than the registry pins is passed over, with a warning, and
   `lsp setup` replaces it.
3. **The system**: the server's command on the `PATH`, and known locations such as
   rustup's toolchains, `~/go/bin`, `~/.npm-global/bin`, LLVM's directories and
   Xcode's toolchain.
4. **Editor extensions**: the language servers bundled with VS Code extensions,
   under `~/.vscode/extensions/` (or, when that does not exist, the extensions
   directory of VS Code Server, Cursor, Windsurf or Code - OSS).

`lsp status`, `detect`, `check` and `setup` report where each server resolves in
places 2 to 4; they do not read the configuration file.

## Installing servers

`devscriptor lsp setup` (or the MCP `lsp_arch_setup` tool with `auto_install`):

1. detects the project's languages: that of each server with a source file
   anywhere in the project, as git sees it (ignore files honoured, hidden
   directories left out), more confidently where the server's project markers are
   there too — a marker alone detects nothing;
2. resolves each language's server as above;
3. checks the prerequisites of each missing server, and names what is missing;
4. installs the missing servers it can — after asking, unless `--yes`;
5. tells you what to install by hand for the others.

A server is installed from the Devscriptor mirror, a GitHub release
(`lsp-servers-<date>-<commit>`) that the build pins with every archive's SHA-256, or from
`DEVSCRIPTOR_LSP_MIRROR_URL`, and checked against that SHA-256
([Building](building.md#the-language-server-mirror)). Without a mirror build for
the platform:

- servers distributed through a package manager are installed with it: npm,
  `go install`, `dotnet tool`, gem, Coursier;
- rust-analyzer, clangd and phpactor come from their GitHub releases, checked
  against the SHA-256 GitHub publishes for each asset;
- jdtls, kotlin-lsp and the Dart SDK come from Eclipse, JetBrains and Google at the
  version the registry pins, checked against the SHA-256 the registry pins (taken
  from the checksum file each publisher puts beside its downloads); the mirror's
  build of these three is taken only when it is that version.

`devscriptor lsp setup <server>…` installs the servers named instead, by id
(`bash-language-server`) or language (`bash`), whether a project needs them or
not, as the MCP `lsp_arch_setup` does with `servers_to_install`; it installs one
on PATH too, as Devscriptor's own, which then comes first. A name that is also a
directory here is the directory.

Devscriptor refuses an unverified download. sourcekit-lsp is never installed: it
comes with the Swift toolchain. A server that fails to install keeps none of the
others from installing, and `lsp setup` then exits with a non-zero status. The
platform scripts `scripts/<platform>/install-dependencies.{sh,ps1}` install the
language runtimes the servers need (see [Installation](installation.md#language-servers)).

`devscriptor lsp check <server>` reports where a server resolves and from which
place, or, when it does not, why it does not run and the prerequisites it lacks,
each with how to install it (`• Go: sudo apt install golang-go  # or:
https://go.dev/dl/`).

## Lifecycle

`devscriptor mcp` answers `initialize` without starting a language server. Each
one starts when it is first needed and stops again when it sits idle. Under a
workspace's shared server ([MCP server](mcp.md#one-server-per-workspace)), the
language servers serve every session of the workspace: each starts once, and is
idle only while no session uses it.

- **Lazy startup** (the default). A tool about a file starts that file's language
  server, rooted at the file's own project. `get_workspace_symbols`,
  `get_project_symbols` and `get_server_capabilities`, which name no file, start
  what they need when no server runs: the servers that apply to the working
  directory, or the named language's.
- **Background start on `lsp_arch_detect`.** `lsp_arch_detect` starts the
  installed servers of the languages it detects in the background, and lists them
  as `prewarming`, so that the first LSP call finds its server running.
- **Eager startup** (`--lsp-startup eager`). Every server that applies to the
  working directory starts before `initialize` is answered. A server applies — here
  and for `lsp_arch_detect` — when its project markers are found within
  `heuristics_max_depth` (3) levels, or a file of its language lies within that
  depth (outside hidden, dependency and build directories). The servers that need
  their project manifest at their root (rust-analyzer, csharp-ls, fsautocomplete,
  metals, graphql-language-service) apply only where their markers are.
- **Folders.** However a server starts, it gets the workspace roots, or, for a
  server that needs its manifest at its root, the projects inside them; for a file
  outside the workspace, the file's project. Such a server, and clangd, is started
  again to take in a project outside its folders.
- **A temporary directory of its own.** Each server runs with `TMPDIR` (`TEMP`,
  `TMP`) set to a directory of its own, `devscriptor-lsp-<pid>-<language>-*` in the
  system's, removed when the server stops, so what a server leaves there goes with
  it. A Devscriptor process killed with SIGKILL removes none; the next one to start
  a server removes those of processes that are gone.
- **Idle shutdown.** A server unused for `idle_timeout_secs` (600) is stopped, and
  the next request starts it again. Unused means no request, notification or
  readiness wait in progress, and none for that long.
- **A cap.** With `max_running_servers` above 0, starting one more server first
  stops the least recently used.

`get_server_status` reports each server as `running`, `exited` or `stopped`, with
a `stop_reason` for a stopped one: `not started`, `idle`, `requested` (by
`stop_server`) or `cap`.

The settings form a `[lifecycle]` table in the configuration file; every key is
optional:

```toml
[lifecycle]
startup = "lazy"             # or "eager"
idle_timeout_secs = 600      # 0 never stops an idle server
max_running_servers = 0      # 0 sets no cap
maintenance_tick_ms = 60000  # how often idle servers are looked for
```

`devscriptor mcp --lsp-startup <lazy|eager>` outranks `DEVSCRIPTOR_LSP_STARTUP`,
which outranks the table, and `DEVSCRIPTOR_LSP_IDLE_SECS` and
`DEVSCRIPTOR_LSP_MAX_SERVERS` outrank theirs. An invalid value in one of the
variables stops `devscriptor mcp` at startup with an error naming it.

## Reliability

Every server starts per file when needed, rooted at that file's project, so the LSP
tools also work on files outside the working directory. These behaviours make the
first request reliable:

- **Files outside a project.** When no project marker is found above a file, the
  server is rooted at the file's own directory, so a loose script is still
  analysed. The languages whose server needs a manifest (Rust, C#, F#, Scala) stay
  strict and say so instead of starting a useless server: *"No LSP server
  configured for language: rust: no project root above …/loose.rs (looked for:
  Cargo.toml, Cargo.lock)"*. GraphQL is strict too, because
  graphql-language-service loads the schema from a GraphQL config: a `.graphql`
  file with none above it fails at once, the message adding
  *"graphql-language-service needs a GraphQL config at the project root"*.
- **Cold-start readiness.** The client advertises `window.workDoneProgress` and
  waits for the server's background indexing to settle before the first semantic
  request. A newly started server is also probed with `documentSymbol` until it has
  loaded the file, so the first `get_hover` or `get_definition` returns real results
  rather than the empty answer of a server still indexing.
- **Capability-aware dispatch.** Document-symbol and workspace-symbol requests are
  checked against the capabilities the server advertised: a server without them
  yields *"the <language> language server does not support …"*, and workspace-symbol
  queries go to every running server that supports them. Every other request goes to
  the server; one that fails for a capability the server did not advertise yields the
  same *"does not support …"*, rather than the server's own error (`-32601 Unhandled
  method`, or jdtls's `-32603 Internal error`). A server that answers a request it did
  not advertise is taken at its word, and the error of one it did advertise is its
  own.
- **Pushed diagnostics.** Servers that only push diagnostics after a file opens or
  changes (typescript-language-server, clangd, gopls, jdtls) have them collected;
  `get_diagnostics` waits for the pushes to settle.

And these keep a session going:

- **Retry when asked.** A request answered with `ContentModified` (`-32801`) or
  `ServerCancelled` (`-32802`) is sent again, up to three times, with a short
  backoff.
- **Restart after a crash.** When a server's process has exited, the next request
  for its language restarts it with the folders it ran with; `get_server_status`
  shows it as `exited`, with the reason in `last_error`, until then. A server that
  exits three times within five minutes is left down until `restart_server` is
  called, so one that dies at every start is not restarted in a loop. Idle and cap
  stops are not crashes and do not count. A request the server was working on when
  it exited fails at once with "Language server closed the connection".
- **A bounded working set.** Each server holds at most 128 documents open; past
  that the least recently used are closed (`didClose`) and opened again from disk if
  asked for.
- **Clean exits.** A language server is shut down on an orderly stop (5 s, then
  killed), and killed from `Drop` when a future is cancelled. On Linux each server
  also asks the kernel for `PR_SET_PDEATHSIG(SIGKILL)`, so it dies with Devscriptor
  even when Devscriptor itself is killed.

## What servers write into a project

- **rust-analyzer** is started with `cargo.targetDir: true`, so it runs Cargo
  (build scripts, proc macros, `cargo check`) in `target/rust-analyzer` instead of
  the project's own `target/`, and never holds the project's build lock: a
  `cargo build` run while it indexes does not wait. The cost is a second set of
  build artifacts in that directory. Its cargo resolves each workspace into
  `target/rust-analyzer/Cargo.lock` (`CARGO_RESOLVER_LOCKFILE_PATH`, Rust 1.97 or
  newer), a copy of the project's `Cargo.lock` where there is one, so a workspace
  without one gets none and an outdated one is not rewritten. A lock file the
  user's environment names is left as it is.
- **jdtls** is started with `-Djava.import.generatesMetadataFilesAtProjectRoot=false`,
  so the Eclipse metadata of a project it imports (`.project`, `.classpath`,
  `.settings/`) stays in its own workspace under `~/.cache/jdtls/`, and with
  `java.autobuild.enabled: false`, so it compiles nothing into the project's output
  folders; it answers from the sources.
- **Metals** is configured with `autoImportBuilds: all`, so it imports a project's
  build, and imports it again when it changes, rather than ask whether to: a client
  with no user to ask can only decline, and Metals without a build answers neither
  workspace symbols nor semantic tokens. It imports an sbt build with `sbt`, which
  must be on `PATH`.
- **Kept out of git:** before Metals starts, the `.metals/` it keeps its database
  and log in and the `.bloop/` and `.bsp/` of the build it imports, and before
  kotlin-lsp or jdtls imports a Gradle build, the `.gradle/` Gradle keeps its project
  cache in, are made with a `.gitignore` that ignores them, as ruby-lsp makes its
  `.ruby-lsp/`. One that exists is left as it is.
- **kotlin-lsp** is started with `--system-path=<cache>/devscriptor/kotlin-lsp`, so
  its caches and indexes persist there rather than in a new temporary directory at
  every start.

The other directories servers write are their build tool's own output —
`target/` and `project/target/`, `obj/` (the .NET servers' restore), `.build/`
(sourcekit-lsp's index build) — which a build of the project makes too; sbt also
writes `project/build.properties` into a build that has none.

## Positions

LSP positions are 0-based, and `character` counts UTF-16 code units, the LSP
default: Devscriptor declares UTF-16 to every server and converts the index's byte
columns to it. Every LSP result with a position also carries `cite`,
`path:line:column` counted from 1, for quoting. The position tools also take a
symbol `name`, which Devscriptor resolves to a position from its index (see
[MCP server](mcp.md#parameter-conventions)).

## Configuration file

Without a configuration file, the LSP tools use every registry server installed on
the machine. To choose the servers yourself, create `devscriptor-lsp.toml` in the
directory you run Devscriptor from, or set `DEVSCRIPTOR_LSP_CONFIG` to a file
anywhere else. That exact filename is the only one searched; there is no fallback
in a configuration directory.

**A project's file must be trusted.** It names commands Devscriptor runs, and the
directory `devscriptor mcp` starts in may be a repository someone else wrote, so a
`devscriptor-lsp.toml` there is ignored, with a warning, until you run:

```bash
devscriptor lsp trust                 # ./devscriptor-lsp.toml, or pass a path
devscriptor lsp untrust               # withdraw it
```

`trust` prints the servers the file would start and records the SHA-256 of that
exact content (in `<config>/devscriptor/trusted-lsp-configs.json`; `<config>` is
`~/.config` on Linux). Editing the file withdraws the trust until you trust it
again. A file named by `DEVSCRIPTOR_LSP_CONFIG` needs no trust: setting the
variable is your own choice.

```toml
[workspace]
roots = ["."]
heuristics_max_depth = 3

# Rust
[[lsp_servers]]
language_id = "rust"
command = "rust-analyzer"
extensions = ["rs"]
heuristics = { markers = ["Cargo.toml", "Cargo.lock"], max_depth = 3 }
initialization_options = { cargo = { features = "all" } }

[lsp_servers.settings]
timeout_seconds = 30
rate_limit_per_minute = 120

# Python
[[lsp_servers]]
language_id = "python"
command = "pyright-langserver"
args = ["--stdio"]
extensions = ["py", "pyw", "pyi"]

# When servers start and stop (optional; see Lifecycle)
[lifecycle]
startup = "lazy"
idle_timeout_secs = 600
```

Each `[[lsp_servers]]` entry takes `language_id` and `command`, and optionally
`args`, `env`, `working_dir`, `extensions`, `heuristics` (project markers),
`initialization_options` (sent with `initialize`), `configuration` (sent to the
server as its settings) and `settings` (Devscriptor's own per-server
`timeout_seconds` and `rate_limit_per_minute`). A complete example for all 21
servers is in [`crates/lsp/config/examples/lsp.toml`](../crates/lsp/config/examples/lsp.toml).

**Request timeouts** are set per server, in `settings.timeout_seconds`. Without a
configuration file the registry's apply: 90 s for volar, sourcekit-lsp and
ruby-lsp, 120 s for jdtls, 180 s for kotlin-lsp, metals, csharp-ls and
fsautocomplete, and 30 s for the rest. A request no answer arrives for is dropped
after 300 s.

The registry itself, `crates/lsp/src/manager/registry.json`, holds each server's
install source, version pin, checksums, arguments and environment; in `args`, `env`
and `initialization_options`, `{server_dir}` is the server's directory and
`{cache_dir}` Devscriptor's cache directory. `crates/lsp/src/generated_registry.rs`
holds the ids, languages, extensions and project markers the bridge routes files
by; a test checks that the two agree.

## CLI commands

```bash
devscriptor lsp setup [PATH] [--yes]      # detect the languages, install the missing servers
devscriptor lsp setup <SERVER>...         # install servers by id or language
devscriptor lsp detect [PATH]             # the languages and their servers, without installing
devscriptor lsp status                    # every known server, grouped by where it resolves
devscriptor lsp check <SERVER_ID>         # one server: where it resolves, or what it lacks
devscriptor lsp batch-check <SERVER_ID>...
devscriptor lsp trust [FILE]              # trust a project's devscriptor-lsp.toml
devscriptor lsp untrust [FILE]

devscriptor definition <FILE> -l <LINE> -c <CHARACTER>   # go to definition (0-based)
devscriptor references <FILE> -l <LINE> -c <CHARACTER>   # find references (0-based)
```

`definition` and `references` ask the live language server for the file's
language; see [CLI](cli.md#definition).

## Troubleshooting

| Problem | What to do |
| --- | --- |
| `No LSP server available for file: No LSP server for extension: .ext` | No installed server handles the file type: `devscriptor lsp setup` (MCP: `lsp_arch_setup`) |
| `No LSP server configured for language: rust: no project root above …` | A language whose server needs a manifest (Rust, C#, F#, Scala), on a file outside any project: use a file inside one |
| `… needs a GraphQL config at the project root` | Add a GraphQL config beside the schema, such as `.graphqlrc.yml` with `schema: schema.graphql` and `documents: "src/**/*.graphql"` |
| `No LSP server configured for language: …` (other) | The server failed to start: run with `--verbose`, and `devscriptor lsp check <server>` |
| `the <language> language server does not support …` | The server does not offer the feature; `get_server_capabilities` shows what it offers |
| `no indexed symbol named '…'` | A `name` with no matching entity in the index: scan the codebase, or pass `file_path`, `line` and `character` |
| A request times out | A slow operation, or a server still indexing: raise `timeout_seconds`, or retry once indexing settles |
| A server is not found | `devscriptor lsp status` shows where each server resolves, or why it does not |

The first request to a server waits for it to start and index: seconds, or minutes
for a large project on rust-analyzer or jdtls. Logs go to stderr
(`devscriptor --verbose lsp status 2> lsp.log`); a workspace's shared MCP server
writes them to its log ([MCP server](mcp.md#runtime-directory-and-logs)).
