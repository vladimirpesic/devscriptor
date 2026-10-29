//! Minimal reproduction harness: run the PRODUCTION parser on a single file.
//!
//! Finds the files that crash tree-sitter (SIGBUS/SIGSEGV) by running the
//! exact production `TreeSitterParser::parse_file` path on one file per process.
//! Because each invocation is its own process, a hardware-signal crash is
//! observable from the parent shell via the exit code:
//!   0   = parsed OK
//!   1   = clean error (skipped/validated)
//!   135 = SIGBUS (128 + 7)
//!   139 = SIGSEGV (128 + 11)
//!
//! Usage: cargo run --release --example parse_one -- <file_path>

use devscriptor_core::parser::{ParserConfig, TreeSitterParser};
use std::path::Path;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: parse_one <file_path>");
        std::process::exit(2);
    }
    let path = Path::new(&args[1]);
    let parser = match TreeSitterParser::new(ParserConfig::default()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parser init failed: {e}");
            std::process::exit(2);
        }
    };

    let sz = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    eprintln!("parsing {} ({} bytes) ...", path.display(), sz);

    match parser.parse_file(path).await {
        Ok(pf) => {
            println!(
                "OK {} bytes -> {} entities, {} relationships",
                sz,
                pf.entities.len(),
                pf.relationships.len()
            );
        }
        Err(e) => {
            eprintln!("ERR {} bytes -> {e}", sz);
            std::process::exit(1);
        }
    }
}
