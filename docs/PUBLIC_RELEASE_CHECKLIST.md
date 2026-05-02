# Public Release Checklist

Use this before publishing a public GitHub repository or cutting a release.

## Repository hygiene

- [ ] `LICENSE` is present.
- [ ] `NOTICE` is present if needed.
- [ ] `README.md` has install, quick start, architecture, and safety notes.
- [ ] `CONTRIBUTING.md` is present.
- [ ] `SECURITY.md` is present.
- [ ] No absolute personal paths are committed.
- [ ] No `.env`, API keys, private tokens, private embeddings, `.cpl/`, or `target/`.
- [ ] `opencode.json` is ignored/local; portable examples live under `examples/`.

## Verification

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --bins
cargo run -- scan --root .
cargo run -- panel --root . "architecture retrieval"
```

## Release notes

- [ ] Update `CHANGELOG.md`.
- [ ] Tag release as `vX.Y.Z`.
- [ ] Attach platform binaries only if built from a clean workflow.
