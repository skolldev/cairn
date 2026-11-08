# cairn

A native desktop git client. See `PRODUCT.md` and `CONTEXT.md`.

## Running

The binary is `cairn`, built by the `cairn-app` crate. It takes the repository to open:

```bash
scripts/fixtures.sh small
cargo run -p cairn-app -- ~/cairn-fixtures/ripgrep
```

`RUST_LOG=info,cairn_repo=debug` shows each Read Job and when its Snapshot is shown.

## Developing

`scripts/install-hooks.sh` runs fmt and clippy before each commit, in this clone and its worktrees.
