# Pins

The exact versions Cairn builds on, why each is pinned, and the checks done before pinning. Upgrading any of these is a deliberate change with its own commit, which updates this file.

| What | Pin | Where |
| --- | --- | --- |
| Rust | 1.98.1 | `rust-toolchain.toml`, and `rust-version` in `Cargo.toml` |
| gix, gix-diff | `main` at `bc4d8135306797b528170709b231faf3e6ebe170` | `Cargo.toml` `[workspace.dependencies]` |
| gpui | `gpui-pre =0.3.11`, with `gpui-pre-platform =0.3.11` | same |
| gpui-component | `gpui-component =0.9.0` | same |
| Everything else | `Cargo.lock` | committed |

Only `cairn-app` may depend on GPUI. `scripts/check-gpui-free.sh` enforces it in CI, for normal, build and dev dependencies on every target, so a test can't pull GPUI in either. `scripts/test-check-gpui-free.sh` runs in CI too. It shows the check failing on deliberate direct, transitive, dev-only and build-only violations, and failing closed when `cargo tree` itself fails. On 2026-11-07 the same deliberate violation was made in the real workspace (`gpui` added to `cairn-core`): the check exited 1 with 84 violations, one per GPUI crate reached through each crate that depends on `cairn-core`.

CI's actions (`actions/checkout@v5`, `Swatinem/rust-cache@v2`) and runners (`macos-latest`, `windows-latest`) follow their tags on purpose. They don't change what Cairn compiles. The toolchain and the lockfile do, and both are pinned.

## Rust 1.98.1

Zed's toolchain at f816e1f, the commit `gpui-pre 0.3.11` republishes (#2). Both dev machines have 1.99.1 as their default (#5). rustup installs 1.98.1 from the toolchain file. Moving to 1.99.1 is an upgrade, not drift.

## gix at bc4d813

The revision #11 measured. Checked 2026-11-07: crates.io's newest gix is still 0.90.0 (2026-10-22), which lacks the topo walk and status fixes (#3). There is no release after 0.90.0 yet, so the pin is the git revision. Move to the first release that contains bc4d813 when it ships. gix-diff comes from the same revision, so the two can't disagree.

## GPUI: re-check against Zed's release plans

Checked 2026-11-07, before pinning.

- **Zed's own releases have not started.** crates.io `gpui` is still 0.2.2 (2025-10-22). In [zed#55271](https://github.com/zed-industries/zed/discussions/55271), the only Zed statement is from a maintainer on 2026-10-30: automated releases on Zed's weekly cadence are being set up, "not quite ready yet", expected "soon". Nothing newer. The last commits to Zed's GPUI publishing tooling (`tooling/xtask/src/tasks/gpui`) are from September 2026.
- **`gpui-pre 0.3.11` is still the newest snapshot** (2026-11-02), and `gpui-component 0.9.0` / `gpui-kit 0.9.0` are still the newest releases that use it.
- **Result: keep `gpui-pre =0.3.11` and `gpui-component =0.9.0`.** Re-check when Zed's first automated GPUI release lands, and before every GPUI upgrade. Moving to Zed's releases removes the single-owner risk of `gpui-pre`.

`gpui-component` is used rather than its umbrella crate `gpui-kit`, because the commit-box spike (#10) proved `gpui-component =0.9.0` with `default-features = false`. The 25 GPUI-family packages in `Cargo.lock` are at exactly the versions and checksums of that spike's lockfile.

## gpui-pre against zed@f816e1f: one-time source diff

Done 2026-11-07. Each `gpui-pre-*` crate in `Cargo.lock` (20 of them, all 0.3.11) was taken from the cargo registry cache, its sha256 checked against `Cargo.lock`, and compared file by file with the crate it names in `[package.metadata.gpui-pre] zed-crate` at zed@`f816e1f9263e3a8ef63c4baaa02e1441ce1c6038` (2026-11-01). Every crate names that same `zed-rev`. Manifests were compared by their declared dependencies, because publishing rewrites `Cargo.toml` and resolves workspace inheritance.

**Result: no unreviewed code reaches Cairn's macOS or Windows builds.** 16 crates have source byte-identical to Zed. Four have changed source, listed below, and every changed file starts with a `// Modified for gpui-pre` comment.

| Crate (Zed source) | Change | Reaches Cairn on macOS / Windows? |
| --- | --- | --- |
| `gpui-pre` (`gpui`) | `actions!` derives `$crate::Action` instead of `gpui::Action`, so the macro works under the renamed crate | Yes. Path fix only |
| `gpui-pre-macros` (`gpui_macros`) | Every proc-macro entry point passes its output through an added `gpui_pre_facade_paths::rewrite`, which rewrites `gpui::` and `gpui_platform::` paths to the `gpui-kit` facade when one is present. Adds the `proc-macro-crate` dependency to find it | Yes. Cairn has no `gpui-kit` facade, so the rewrite returns tokens unchanged |
| `gpui-pre-apple` (`gpui_apple`) | `build.rs` reads the gpui sources it needs from `vendor/gpui` instead of `../gpui`. The five vendored files are byte-identical to zed@f816e1f `crates/gpui/src` | macOS only. Path fix only |
| `gpui-pre-wgpu` (`gpui_wgpu`) | Adapted to crates.io wgpu 30.0.1 instead of Zed's wgpu git revision: drops `default_queue` from two device descriptors, and uses `is_srgb()` in place of `has_srgb_suffix()` | No. Only `gpui-pre-web` uses it |

Ten manifests drop dev-dependencies, which never build for a dependent. Two crates omit one file each that Zed has; packaging excludes files, so that adds nothing. `gpui-pre-reqwest` (0.12.15) republishes Zed's reqwest fork rather than a crate from the Zed repo, so it is outside this diff. It is not in the macOS or Windows dependency graph.

Repeat this diff for every `gpui-pre` upgrade until Cairn moves to Zed's own releases.

## After a `gpui-pre` upgrade: the macOS terminate guard

`crates/app/src/terminate.rs` adds `applicationShouldTerminate:` to GPUI's private `GPUIApplicationDelegate` class, and GPUI's test platform can't reach it. Check by hand that an app-level quit still takes the close flow: run `RUST_LOG=info target/debug/cairn <repo>`, then send it a quit as the Dock does:

```bash
osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier(<pid>).terminate"
```

It passes if the trace shows `kind="quit"` before the app exits. An app that exits with no such line has lost the guard, and an error at launch means the class has gone.
