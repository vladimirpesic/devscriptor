# Security policy

## Supported versions

Fixes go into the latest release: a vulnerability is fixed in a new release,
whose notes say what it fixes.

## Reporting a vulnerability

Report a vulnerability privately, never in a public issue:

- with GitHub's private vulnerability reporting: **Security → Report a
  vulnerability** on this repository
  ([open a report](https://github.com/vladimirpesic/devscriptor/security/advisories/new)),
  or
- by email to **<contact@devscriptor.com>**.

Give the Devscriptor version (`devscriptor --version`), the platform, and what
shows the problem: the command or MCP tool call, the files involved, and what
Devscriptor did.

## What is a vulnerability in Devscriptor

[Security](docs/security.md) describes what Devscriptor trusts and what it
verifies: the files it reads, the embedding model and language servers it
downloads, each checked against a pinned SHA-256, and the language servers a
project's configuration can start. A vulnerability is a way around that, for
example:

- a file read or written outside the path given to `scan` or `watch`, or outside
  an archive's destination when a download is unpacked;
- a download used without its checksum matching;
- a crash, or memory corrupted, by a file it parses or a request to its MCP
  server.
