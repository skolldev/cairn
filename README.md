# cairn

A native desktop git client. See `PRODUCT.md` and `CONTEXT.md`.

## Running

The binary is `cairn`, built by the `cairn-app` crate. It takes the repository to open:

```bash
scripts/fixtures.sh small
cargo run -p cairn-app -- ~/cairn-fixtures/ripgrep
```

`RUST_LOG=info,cairn_repo=debug` shows each Read Job and when its Snapshot is shown.

## Measuring Budgets

`scripts/budget.sh run` measures the Phase 1 Budgets on the real binary. `docs/budget-harness.md` has the rules, what each number means and the Windows setup.
