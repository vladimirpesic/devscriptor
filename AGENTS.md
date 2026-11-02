# Working on Devscriptor

Devscriptor is a Cargo workspace of four crates — `devscriptor-core` (parsing, the
database, the pipeline, search, embeddings, analysis), `devscriptor-lsp`
(language-server detection, installation and the LSP bridge), `devscriptor-mcp`
(the MCP server and its tools) and `devscriptor-cli` (the `devscriptor` binary) —
plus a VS Code extension in `vscode-extension/`. [docs/architecture.md](docs/architecture.md)
describes how they fit together, and [docs/decisions.md](docs/decisions.md) why.

## Definition of done

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three pass, with no new warning from rustc, clippy or rust-analyzer. A change
to the extension also passes `npm run lint` and `npm run test:unit` in
`vscode-extension/`. [docs/building.md](docs/building.md#tests) lists the slow and
ignored suites.

## Rules

- **Docs follow the code.** A change that alters behaviour updates every document
  that describes it, in the same change. The tool tables in `docs/mcp.md` mirror the
  server's `tools/list`, and the command tables in `docs/cli.md` the binary's
  `--help`.
- **Comments and docs describe the code as it is** — what it does and why — not how
  it came to be.
- **stdout belongs to the protocol** in `devscriptor mcp`: log with `tracing`
  (stderr), never `println!`, on an MCP code path.
- **Extraction changes** update the golden sidecars deliberately:
  `cargo test -p devscriptor-core --test golden_corpus -- --ignored generate_golden_sidecars`,
  then review the changed entity lists before committing them.
- **A test that needs the real embedding model** is gated behind the `slow-tests`
  feature; the others use `MockEmbeddingProvider`.

## Technology

| Component | Crate | Version |
| --- | --- | --- |
| Parsing | `tree-sitter` and one `tree-sitter-*` crate per grammar | 0.27 (26 grammar crates, 27 parsers) |
| Database | `rusqlite`, with SQLite bundled | 0.40 |
| Vector search | `sqlite-vec` | 0.1 |
| Embeddings | `tract-onnx`, pure-Rust ONNX inference | 0.23 |
| Matrix threads | `tract-linalg` with `multithread-mm` (rayon) | 0.23 |
| Async runtime | `tokio` | 1 |
| MCP protocol | `rmcp` | 3.5 |
| File watching | `notify` | 8 |
| CLI | `clap` | 4 |
| Serialization | `serde` | 1 |
| Errors | `thiserror` / `anyhow` | 2 / 1 |
