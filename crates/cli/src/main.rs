//! Devscriptor CLI - High-performance code analysis tool
//!
//! This is the main entry point for the Devscriptor CLI application.
//! Commands are organized into submodules under the `commands` directory.

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use devscriptor_core::analysis::Severity;
use std::path::PathBuf;

use commands::{
    AnalysisKind, CallGraphFormat, OutputFormat, ReportFormat, SearchType,
    context::ContextCommands, print_banner,
};

#[derive(Parser)]
#[command(name = "devscriptor")]
#[command(about = "High-performance code analysis tool")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Database file path.
    ///
    /// Defaults to `devscriptor.db` in the working directory. `devscriptor mcp`
    /// instead defaults to a shared, location-independent database, so a server
    /// started from anywhere opens the same index. Naming a path overrides both.
    // No clap `default_value`: it would print `[default: devscriptor.db]` in
    // `devscriptor mcp --help`, which is not the database that command opens,
    // and would have callers wire the MCP server to an index it never reads.
    // Each command applies its own default instead; see `project_database`.
    #[arg(short, long, global = true)]
    database: Option<PathBuf>,

    /// Project configuration file.
    ///
    /// Defaults to `devscriptor.yaml` in the project's directory: the path
    /// `scan` and `watch` index, the workspace of `mcp` and `serve`, the
    /// working directory for the other commands. Flags and environment
    /// variables override its settings.
    // Long only: `-c` is `definition`'s and `references`' `--character`.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a codebase and build the knowledge graph.
    ///
    /// By default the scan is **fast** (FTS5 keyword search available
    /// immediately) because embedding is skipped.  To add semantic vector
    /// search, either pass `--embed` for inline embedding, or run
    /// `devscriptor embed` separately afterwards.
    Scan {
        /// Path to the codebase to scan
        path: PathBuf,
        /// File extensions to include (comma-separated, e.g. `rs,py`);
        /// replaces the project's `scan.include`
        #[arg(short, long, value_delimiter = ',')]
        include: Vec<String>,
        /// Glob patterns to exclude (comma-separated); replaces the
        /// project's `scan.exclude`
        #[arg(short, long, value_delimiter = ',')]
        exclude: Vec<String>,
        /// Maximum file size in MB (default: the project's, else 50 MB)
        #[arg(long)]
        max_file_size: Option<usize>,
        /// File processing batch size
        #[arg(long, default_value = "50")]
        batch_size: usize,
        /// Skip unchanged files (requires a previous scan of the same database)
        #[arg(long)]
        incremental: bool,
        /// Generate semantic embedding vectors inline after scanning.
        /// The quantized ONNX model is loaded, embeddings are generated, and the
        /// model is unloaded immediately.  Runs within ~4 GB free RAM.
        /// Skipped automatically if RAM is insufficient.
        #[arg(long)]
        embed: bool,
        /// Force-skip embeddings even if --embed is passed.
        /// Useful for CI / resource-constrained environments.
        #[arg(long)]
        skip_embeddings: bool,
        /// Use bulk-load FTS mode (drops FTS5 triggers, faster for fresh
        /// full scans, not safe for incremental).
        #[arg(long)]
        bulk_load: bool,
        /// Print an extended quality-metrics summary after scanning:
        /// full entity/language distribution, embedding cache-hit rate, peak RSS.
        #[arg(long)]
        metrics: bool,
    },

    /// Watch a codebase and keep the knowledge graph current in real time.
    ///
    /// Runs an incremental re-scan whenever source files change (create /
    /// modify / delete / rename). Runs until interrupted with Ctrl-C.
    Watch {
        /// Path to the codebase to watch
        path: PathBuf,
        /// File extensions to include (comma-separated, e.g. `rs,py`);
        /// replaces the project's `scan.include`
        #[arg(short, long, value_delimiter = ',')]
        include: Vec<String>,
        /// Glob patterns to exclude (comma-separated); replaces the
        /// project's `scan.exclude`
        #[arg(short, long, value_delimiter = ',')]
        exclude: Vec<String>,
        /// Re-embed changed entities inline after each re-scan (requires ~4 GB RAM)
        #[arg(long)]
        embed: bool,
        /// Debounce window in milliseconds (coalesces bursts of edits)
        #[arg(long, default_value = "400")]
        debounce_ms: u64,
    },

    /// Analyze code for various metrics
    Analyze {
        /// Path to the codebase
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Type of analysis to run
        #[arg(short, long, value_enum)]
        kind: AnalysisKind,
        /// Minimum severity to report
        #[arg(long, value_enum, default_value = "medium")]
        severity: Severity,
        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        output: OutputFormat,
    },

    /// Search the codebase
    Search {
        /// Query string
        query: String,
        /// Search type
        #[arg(short, long, value_enum, default_value = "symbol")]
        search_type: SearchType,
        /// Maximum results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Output format: text, or a JSON array of hits
        #[arg(short, long, value_enum, default_value = "text")]
        output: OutputFormat,
    },

    /// Query database status (with optional project path)
    Status {
        /// Optional path to codebase for contextual stats
        #[arg(short, long)]
        path: Option<PathBuf>,
    },

    /// Clean database and start fresh
    Clean,

    /// Template management commands
    Template {
        #[command(subcommand)]
        command: TemplateCommands,
    },

    /// Generate function call graph
    CallGraph {
        /// Target entity to start from
        #[arg(short, long)]
        entity: String,
        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: CallGraphFormat,
        /// Maximum depth
        #[arg(short, long, default_value = "3")]
        max_depth: usize,
        /// Direction (incoming, outgoing, both)
        //
        // No `short`: `-d` is reserved by the global `--database` flag; clap
        // rejects duplicate short options. Use the long form `--direction`.
        #[arg(long, default_value = "both")]
        direction: String,
    },

    /// Generate architecture report
    ArchitectureReport {
        /// Path to the codebase
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: ReportFormat,
        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Validate a project configuration file
    Validate {
        /// The file to validate (default: `--config`, else `devscriptor.yaml`)
        config_file: Option<PathBuf>,
    },

    /// Search for functions
    SearchFunction {
        /// Function name to search for
        name: String,
        /// Perform fuzzy search
        #[arg(short, long)]
        fuzzy: bool,
        /// Maximum number of results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Search for classes
    SearchClass {
        /// Class name to search for
        name: String,
        /// Perform fuzzy search
        #[arg(short, long)]
        fuzzy: bool,
        /// Maximum number of results
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Find similar entities
    FindSimilar {
        /// Query text to find similar entities
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Search entities by pattern
    SearchPattern {
        /// Pattern to search for
        pattern: String,
        /// Entity type to search
        #[arg(short = 't', long)]
        entity_type: Option<String>,
        /// Language filter
        #[arg(short, long)]
        language: Option<String>,
    },

    /// Show parser cache statistics
    CacheStats,

    /// Show performance metrics
    PerfStats,

    /// Configuration profile management
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },

    /// Interactive REPL mode
    Repl,

    /// LSP server management (requires 'lsp' feature)
    #[cfg(feature = "lsp")]
    Lsp {
        #[command(subcommand)]
        command: commands::lsp::LspCommands,
    },

    /// Context graph management
    Context {
        #[command(subcommand)]
        command: ContextCommands,
    },

    /// Generate semantic embedding vectors for all entities in the database.
    ///
    /// Run this after `devscriptor scan` to enable semantic (vector) search.
    /// The quantized embedding model is loaded, all un-vectorized entities are
    /// processed, and the model is immediately unloaded to free its RAM (~1.3 GB).
    ///
    /// This command is idempotent: re-running it only processes entities that
    /// still lack vectors.
    ///
    /// Flags:
    ///   --daemon   Fork a background process and return immediately.
    ///              Progress is written to the `embedding_jobs` table and
    ///              readable via `devscriptor embed --status`.
    ///   --status   Print the latest background job progress and exit.
    Embed {
        /// Entities per inference batch. 0 = auto (sized to free RAM and cores,
        /// worst-case-safe for long entities); set explicitly to override.
        #[arg(long, default_value = "0")]
        batch_size: usize,
        /// Re-embed entities that already have vectors
        #[arg(long)]
        force: bool,
        /// Fork a background embedding process and return immediately.
        /// Progress is tracked in the `embedding_jobs` table.
        #[arg(long, conflicts_with_all = ["status", "run_daemon"])]
        daemon: bool,
        /// Show progress of the latest background embedding job.
        #[arg(long, conflicts_with_all = ["daemon", "run_daemon"])]
        status: bool,
        /// Internal: run the embedding loop inside a daemon process (do not use directly).
        #[arg(long, hide = true, conflicts_with_all = ["daemon", "status"])]
        run_daemon: bool,
    },

    /// Run health checks
    Doctor,

    /// Output database statistics as JSON
    Stats,

    /// Go to definition of a symbol at a file position (requires live LSP server)
    #[cfg(feature = "lsp")]
    Definition {
        /// Absolute path to the source file
        file_path: String,
        /// Line number (0-indexed)
        #[arg(short, long)]
        line: u32,
        /// Character position (0-indexed)
        #[arg(short, long)]
        character: u32,
    },

    /// Find all references to a symbol (requires live LSP server)
    #[cfg(feature = "lsp")]
    References {
        /// Absolute path to the source file
        file_path: String,
        /// Line number (0-indexed)
        #[arg(short, long)]
        line: u32,
        /// Character position (0-indexed)
        #[arg(short, long)]
        character: u32,
        /// Include the declaration in results: on by default, and
        /// `--include-declaration false` turns it off
        #[arg(
            long,
            default_value_t = true,
            num_args = 0..=1,
            default_missing_value = "true",
            action = clap::ArgAction::Set
        )]
        include_declaration: bool,
    },

    /// Launch the devscriptor MCP server (JSON-RPC over stdio, requires 'mcp' feature)
    #[cfg(feature = "mcp")]
    Mcp {
        /// When language servers start: `lazy` (default), each when a tool
        /// first needs it or `lsp_arch_detect` finds its language; `eager`, all
        /// that apply before the server answers. Overrides
        /// `DEVSCRIPTOR_LSP_STARTUP` and the `[lifecycle]` table of
        /// `devscriptor-lsp.toml`.
        #[arg(long, value_enum)]
        lsp_startup: Option<commands::mcp::LspStartup>,
        /// Do not watch the scanned project the server runs in. By default the
        /// index follows file changes there; `DEVSCRIPTOR_WATCH=0` does the same.
        #[arg(long)]
        no_watch: bool,
        /// Serve in this process instead of relaying to the workspace's
        /// shared daemon (`devscriptor serve`); `DEVSCRIPTOR_DAEMON=0` does the
        /// same.
        #[arg(long)]
        no_daemon: bool,
    },

    /// Serve a workspace's MCP sessions from one process (requires 'mcp' feature).
    ///
    /// Holds the index, its watcher, the embedding model and the language
    /// servers once, and serves every MCP client of the workspace over a local
    /// socket, one session per connection. The socket, lock, sidecar and log
    /// are in `$XDG_RUNTIME_DIR/devscriptor/` (else `~/.cache/devscriptor/run/`).
    /// Stops on SIGTERM or Ctrl-C.
    #[cfg(feature = "mcp")]
    Serve {
        /// The workspace to serve: its scanned root is watched.
        #[arg(long)]
        workspace: PathBuf,
        /// When language servers start: `lazy` (default) or `eager`, as for
        /// `devscriptor mcp`.
        #[arg(long, value_enum)]
        lsp_startup: Option<commands::mcp::LspStartup>,
        /// Do not watch the workspace's scanned root.
        #[arg(long)]
        no_watch: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// List available configuration profiles
    List,
    /// Show profile details
    Show { name: String },
    /// Create a new profile
    Create {
        name: String,
        #[arg(short, long)]
        based_on: Option<String>,
    },
    /// Apply a profile to the current configuration
    Apply {
        name: String,
        #[arg(short, long, default_value = "devscriptor.yaml")]
        output: PathBuf,
    },
    /// Delete a profile
    Delete { name: String },
}

#[derive(Subcommand)]
enum TemplateCommands {
    /// List available templates
    List {
        #[arg(short, long)]
        category: Option<String>,
        #[arg(short, long)]
        search: Option<String>,
    },
    /// Show template details
    Show { name: String },
    /// Apply a template
    Apply {
        name: String,
        #[arg(short, long, default_value = "devscriptor.yaml")]
        output: PathBuf,
    },
    /// Analyze a project: its stacks, the template that suits it, and the
    /// configuration they suggest
    Analyze {
        project_path: PathBuf,
        /// Write the suggested configuration to this file
        /// (e.g. `devscriptor.yaml`)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Create a new template
    Create {
        name: String,
        #[arg(short, long)]
        based_on: Option<String>,
    },
    /// Export a template
    Export {
        name: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    // There is exactly one way to start the MCP server: `devscriptor mcp`. One
    // program answering to one name works on every platform; an alternate name
    // needs a symlink, which dangles when the binary is installed on its own, or
    // on Windows a `.cmd` shim, which Node's `child_process.spawn` cannot start
    // without `shell: true` and Rust's `std::process::Command` not at all.
    if let Err(e) = run().await {
        // Release builds redact filesystem paths from surfaced errors, so absolute
        // paths never reach users (see docs/privacy.md); in debug builds
        // `sanitize_message` passes the message through, for full diagnostics.
        eprintln!(
            "Error: {}",
            devscriptor_core::sanitize_message(&format!("{e:?}"))
        );
        std::process::exit(1);
    }
}

/// The database path for a command that works on a project-local index.
///
/// `devscriptor mcp` deliberately does not use this: see [`Cli::database`].
fn project_database(database: &Option<PathBuf>) -> PathBuf {
    database
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PROJECT_DATABASE))
}

/// The index `devscriptor scan` writes in the directory it is run from.
const DEFAULT_PROJECT_DATABASE: &str = "devscriptor.db";

async fn run() -> Result<()> {
    // `--database` carries no clap default, so `Option::is_some` already tells an
    // explicit path from an unset one — `devscriptor mcp` needs that distinction
    // to fall back to its shared database.
    let cli = Cli::parse();

    // Initialize logging
    // Quiet by default so a command's formatted output isn't polluted by tracing
    // lines; `--verbose` opts into full debug logging. `r2d2=off`: the pool
    // logs, and retries, every failure to open a connection; one that matters
    // still surfaces through our own `?`-propagated errors.
    // `rmcp::service=error`: rmcp logs every failed request at WARN, although
    // the error already goes back to the client in the response — under
    // `devscriptor mcp` that doubled each failed tool call as a line in the
    // client's logs.
    let filter = if cli.verbose {
        "debug,r2d2=off"
    } else {
        "warn,r2d2=off,rmcp::service=error"
    };
    // Always write logs to stderr. `devscriptor mcp` serves JSON-RPC over stdout,
    // so a tracing line on stdout (e.g. the LSP "failed to spawn server" warning)
    // corrupts the MCP stream and kills the transport. Keeping stderr for logs is
    // also the conventional split for every other command.
    // Colour only for a terminal: under `devscriptor mcp` stderr is a pipe
    // to the client, and escape codes land in its logs.
    let stderr_is_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(stderr_is_terminal);
    // `devscriptor serve` runs for hours, mostly detached: it logs at info to
    // its own rotating log, and to stderr too when a person runs it there. A
    // daemon the proxy starts has its stderr on that log already.
    #[cfg(feature = "mcp")]
    let subscriber = {
        use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt as _};
        let writer = match &cli.command {
            Commands::Serve { workspace, .. } => {
                let log = commands::serve::log(workspace, cli.database.clone())?;
                Some(if stderr_is_terminal {
                    BoxMakeWriter::new(log.and(std::io::stderr))
                } else {
                    BoxMakeWriter::new(log)
                })
            }
            _ => None,
        };
        let serving = writer.is_some();
        let writer = writer.unwrap_or_else(|| BoxMakeWriter::new(std::io::stderr));
        subscriber
            .with_env_filter(match (serving, cli.verbose) {
                (true, false) => "info,r2d2=off,rmcp::service=error",
                _ => filter,
            })
            .with_writer(writer)
            .with_ansi(stderr_is_terminal && !serving)
    };
    if let Err(e) = subscriber.try_init() {
        // This usually means a subscriber was already initialized (e.g. by a test
        // harness).  Log the failure at debug level via stderr so that it is
        // visible during troubleshooting but does not pollute normal output.
        eprintln!("[debug] tracing_subscriber::try_init() failed: {e}");
    }

    // Only print banner for non-interactive commands.
    // `Mcp` must never print here: its stdout carries the JSON-RPC stream.
    match &cli.command {
        Commands::Repl | Commands::Profile { .. } | Commands::Context { .. } => {}
        #[cfg(feature = "lsp")]
        Commands::Lsp { .. } => {}
        #[cfg(feature = "mcp")]
        Commands::Mcp { .. } | Commands::Serve { .. } => {}
        _ => print_banner(),
    }

    // Open one shared database for the commands that need an existing one, so
    // they share its connection pool.
    let shared_db = match &cli.command {
        Commands::Analyze { .. }
        | Commands::Search { .. }
        | Commands::Status { .. }
        | Commands::CallGraph { .. }
        | Commands::SearchFunction { .. }
        | Commands::SearchClass { .. }
        | Commands::FindSimilar { .. }
        | Commands::SearchPattern { .. }
        | Commands::ArchitectureReport { .. }
        | Commands::Repl
        | Commands::Context { .. } => {
            // These commands all read an existing knowledge graph. Opening a
            // missing path would silently create an empty database and produce
            // confusing "0 results"; give a clear next step instead.
            if !project_database(&cli.database).exists() {
                eprintln!(
                    "No database found at '{}'.\nRun `devscriptor scan <path>` first to build the knowledge graph.",
                    project_database(&cli.database).display()
                );
                std::process::exit(1);
            }
            let project = commands::common::working_directory_config(cli.config.as_deref())?;
            Some(commands::common::open_database(
                &project_database(&cli.database),
                &project,
            )?)
        }
        _ => None,
    };

    match cli.command {
        Commands::Scan {
            path,
            include,
            exclude,
            max_file_size,
            batch_size,
            incremental,
            embed,
            skip_embeddings,
            bulk_load,
            metrics,
        } => {
            let project = commands::common::find_project_config(cli.config.as_deref(), &path)?;
            let args = commands::scan::ScanArgs {
                path,
                database: project_database(&cli.database),
                project,
                include,
                exclude,
                max_file_size,
                batch_size: Some(batch_size),
                incremental,
                embed,
                skip_embeddings,
                bulk_load,
                // Unload model right after scan — this is a one-shot CLI call.
                unload_model_after_scan: true,
                metrics,
            };
            commands::scan::execute(args).await?;
        }
        Commands::Watch {
            path,
            include,
            exclude,
            embed,
            debounce_ms,
        } => {
            let project = commands::common::find_project_config(cli.config.as_deref(), &path)?;
            let args = commands::watch::WatchArgs {
                path,
                database: project_database(&cli.database),
                project,
                include,
                exclude,
                embed,
                debounce_ms,
            };
            commands::watch::execute(args).await?;
        }
        Commands::Analyze {
            path,
            kind,
            severity,
            output,
        } => {
            let path = path.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            let severity = Some(severity);
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::analyze::execute(path, db, kind, output, severity).await?;
        }
        Commands::Search {
            query,
            search_type,
            limit,
            output,
        } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::search::execute(query, db, search_type, limit, output).await?;
        }
        Commands::Status { path } => {
            commands::database::status(
                shared_db.ok_or_else(|| {
                    anyhow::anyhow!("shared database should be open for this command")
                })?,
                path,
            )
            .await?;
        }
        Commands::Clean => {
            commands::database::clean(project_database(&cli.database)).await?;
        }
        // Synchronous handlers run in `spawn_blocking`, off the async runtime.
        Commands::Template { command } => match command {
            TemplateCommands::List { category, search } => {
                tokio::task::spawn_blocking(move || commands::template::list(category, search))
                    .await??;
            }
            TemplateCommands::Show { name } => {
                tokio::task::spawn_blocking(move || commands::template::show(name)).await??;
            }
            TemplateCommands::Apply { name, output } => {
                tokio::task::spawn_blocking(move || commands::template::apply(name, output))
                    .await??;
            }
            TemplateCommands::Analyze {
                project_path,
                output,
            } => {
                tokio::task::spawn_blocking(move || {
                    commands::template::analyze(project_path, output)
                })
                .await??;
            }
            TemplateCommands::Create { name, based_on } => {
                tokio::task::spawn_blocking(move || commands::template::create(name, based_on))
                    .await??;
            }
            TemplateCommands::Export { name, output } => {
                tokio::task::spawn_blocking(move || commands::template::export(name, output))
                    .await??;
            }
        },
        Commands::CallGraph {
            entity,
            format,
            max_depth,
            direction,
        } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::call_graph::execute(entity, db, format, max_depth, direction).await?;
        }
        Commands::ArchitectureReport {
            path,
            format,
            output,
        } => {
            let path = path.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            let db = shared_db;
            commands::architecture::execute(path, db, format, output).await?;
        }
        Commands::Validate { config_file } => {
            commands::validate::execute(config_file, cli.config.clone())?;
        }
        Commands::SearchFunction { name, fuzzy, limit } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::search::search_function(name, db, fuzzy, limit).await?;
        }
        Commands::SearchClass { name, fuzzy, limit } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::search::search_class(name, db, fuzzy, limit).await?;
        }
        Commands::FindSimilar { query, limit } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::search::find_similar(query, db, limit).await?;
        }
        Commands::SearchPattern {
            pattern,
            entity_type,
            language,
        } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::search::search_pattern(pattern, db, entity_type, language).await?;
        }
        Commands::CacheStats => {
            tokio::task::spawn_blocking(commands::cache::stats).await??;
        }
        Commands::PerfStats => {
            tokio::task::spawn_blocking(commands::performance::stats).await??;
        }
        Commands::Profile { command } => match command {
            ProfileCommands::List => {
                tokio::task::spawn_blocking(commands::profile::list).await??;
            }
            ProfileCommands::Show { name } => {
                tokio::task::spawn_blocking(move || commands::profile::show(name)).await??;
            }
            ProfileCommands::Create { name, based_on } => {
                tokio::task::spawn_blocking(move || commands::profile::create(name, based_on))
                    .await??;
            }
            ProfileCommands::Apply { name, output } => {
                tokio::task::spawn_blocking(move || commands::profile::apply(name, output))
                    .await??;
            }
            ProfileCommands::Delete { name } => {
                tokio::task::spawn_blocking(move || commands::profile::delete(name)).await??;
            }
        },
        Commands::Repl => {
            commands::repl::start(shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?)
            .await?;
        }
        #[cfg(feature = "lsp")]
        Commands::Lsp { command } => {
            commands::lsp::handle_lsp_command(command).await?;
        }
        Commands::Context { command } => {
            let db = shared_db.ok_or_else(|| {
                anyhow::anyhow!("shared database should be open for this command")
            })?;
            commands::context::execute(db, command).await?;
        }
        Commands::Embed {
            batch_size,
            force,
            daemon,
            status,
            run_daemon,
        } => {
            let args = commands::embed::EmbedArgs {
                database: project_database(&cli.database),
                project: commands::common::working_directory_config(cli.config.as_deref())?,
                config: cli.config.clone(),
                batch_size,
                force,
                daemon,
                status,
            };
            if run_daemon {
                commands::embed::run_embed(args, None).await?;
            } else {
                commands::embed::execute(args).await?;
            }
        }
        Commands::Doctor => {
            commands::doctor::execute(project_database(&cli.database), cli.config.clone()).await?;
        }
        Commands::Stats => {
            let project = commands::common::working_directory_config(cli.config.as_deref())?;
            commands::database::execute_stats(project_database(&cli.database), &project).await?;
        }
        #[cfg(feature = "lsp")]
        Commands::Definition {
            file_path,
            line,
            character,
        } => {
            commands::lsp::execute_definition(
                project_database(&cli.database),
                file_path,
                line,
                character,
            )
            .await?;
        }
        #[cfg(feature = "lsp")]
        Commands::References {
            file_path,
            line,
            character,
            include_declaration,
        } => {
            commands::lsp::execute_references(
                project_database(&cli.database),
                file_path,
                line,
                character,
                include_declaration,
            )
            .await?;
        }
        #[cfg(feature = "mcp")]
        Commands::Mcp {
            lsp_startup,
            no_watch,
            no_daemon,
        } => {
            // Only an explicit `--database` overrides the server's own default;
            // the global default (`./devscriptor.db`) must not leak in here.
            let db_path = cli.database.clone();
            commands::mcp::execute_mcp(
                db_path,
                cli.config.clone(),
                lsp_startup,
                no_watch,
                no_daemon,
            )
            .await?;
        }
        #[cfg(feature = "mcp")]
        Commands::Serve {
            workspace,
            lsp_startup,
            no_watch,
        } => {
            commands::serve::execute_serve(
                workspace,
                cli.database.clone(),
                cli.config.clone(),
                lsp_startup,
                no_watch,
            )
            .await?;
        }
    }

    Ok(())
}

#[cfg(all(test, feature = "lsp"))]
mod tests {
    use super::*;

    fn include_declaration(args: &[&str]) -> bool {
        let argv = [
            &["devscriptor", "references", "/p/a.rs", "-l", "1", "-c", "2"],
            args,
        ]
        .concat();
        match Cli::try_parse_from(argv)
            .expect("valid command line")
            .command
        {
            Commands::References {
                include_declaration,
                ..
            } => include_declaration,
            _ => unreachable!("parsed as `references`"),
        }
    }

    /// `--include-declaration` is on by default and takes `true` or `false`, so
    /// it can be turned off.
    #[test]
    fn include_declaration_is_on_by_default_and_can_be_turned_off() {
        assert!(include_declaration(&[]));
        assert!(include_declaration(&["--include-declaration"]));
        assert!(include_declaration(&["--include-declaration", "true"]));
        assert!(!include_declaration(&["--include-declaration", "false"]));
    }
}
