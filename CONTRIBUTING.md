# Contributing to Devscriptor

Issues and pull requests are welcome. A security problem goes to the private
channels in [SECURITY.md](SECURITY.md), not to an issue.

## Before a pull request

[Building](docs/building.md) sets up the toolchain, and [AGENTS.md](AGENTS.md)
gives the workspace's layout and the rules its code follows. A change is ready
when these pass, with no new warning, as CI runs them on Linux, macOS and
Windows:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

A change to the VS Code extension also passes, in `vscode-extension/`:

```bash
npm audit
npm run lint
npm run test:unit
```

[Building](docs/building.md#tests) lists the slow and ignored suites, and the
extension tests that run in VS Code.

## Release notes

Each release's notes are its changelog; there is no other. They are
`.github/release-notes/v<version>.md`, written with the version bump, and the
release workflow publishes them as the release's description: a tag whose commit
has none builds nothing. They tell someone who uses Devscriptor what changed for
them — what is new, what is fixed, what to do on upgrading — not how the code
changed.

## License

Devscriptor is licensed under the GPL-3.0-only. A contribution is licensed under
the same terms.
