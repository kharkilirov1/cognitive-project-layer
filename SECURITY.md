# Security Policy

Cognitive Project Layer is a local-first developer tool that reads project files
and exposes project context through CLI, MCP stdio, and an optional local HTTP
server.

## Supported versions

Security fixes target the latest `main` branch until stable releases are cut.

## Reporting vulnerabilities

Please report security issues privately before opening a public issue.

If the repository has GitHub private vulnerability reporting enabled, use that
channel. Otherwise, contact the maintainer through the repository owner profile.

## Security expectations

- Do not expose the HTTP server on a public interface unless you understand the risk.
- Keep `serve` bound to `127.0.0.1` unless intentionally testing another setup.
- Do not commit `.env`, API keys, `.cpl/vector_db.json`, or private embeddings.
- Review MCP server configs before enabling them in an agent host.
- Treat third-party MCP clients and servers as code execution boundaries.

## Local data

The default persistent embedding DB lives in `.cpl/vector_db.json` and may contain
chunks derived from your source code. It is ignored by default and should not be
published unless you intentionally generated it for a public repository.

