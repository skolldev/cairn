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

## GPUI's dispatcher and the lanes' QoS

Checked 2026-11-08 at `gpui-pre 0.3.11`, in `gpui-pre-apple`'s `dispatcher.rs`, before choosing QoS classes for the `repo` crate's lanes (ADR 0001). GPUI runs UI work on the main thread (user-interactive), and sends High, Medium and Low background work to libdispatch's global queues of those priorities, which map to the user-initiated, default and utility classes. So the Interactive lane runs at user-initiated, level with GPUI's High work, and the Background lane at utility, level with its Low work. Neither competes with drawing. Re-check this on any GPUI upgrade. `Qos` in `crates/repo/src/scheduler.rs` holds the choice. A lane's QoS covers its own thread only: nothing here checks what class a git process it starts runs at.

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

## gpui-component's font check costs Launch about 140 ms on macOS

Found 2026-11-10 at `gpui-component =0.9.0` (#83). `gpui_component::init` takes about 148 ms on the M3 Max, before Cairn's window exists, and almost all of it is `theme::init`. Its `Theme::change` checks that `.SystemUIFont` and the default monospace family (Menlo on macOS) are installed, so it can fall back when they're not. To check, it lists every installed font through GPUI's `all_font_names()` (`theme/mono_font.rs`, `installed_font_names`, also used by `theme/system_font.rs`).

- **The listing is the cost, and no one-off warm-up.** It took about 144 ms each time it ran, even straight after another listing. Loading `.SystemUIFont` itself, which is all the first paint needs, took 3 to 5 ms.
- **Without it, Launch passes.** A throwaway build whose `installed_font_names` returned what this Mac's listing finds of those families moved every phase after the window about 135 ms earlier, the first presented frame included. `trace --only launch` read 245.4 ms against 378.6 ms. `scripts/budget.sh run --only launch` passed the Budget of the day: 249.4 ms against under 300, worst sample 304.1 ms against 400. The stock build missed on both counts the same day. The plan moved Launch to under 400 ms for this reason, until a fix lands.
- **Moving it wouldn't do it.** Cairn calls `gpui_component::init` before opening the window. Calling it after the first paint would still block the main thread before the first Snapshots are drained, and the frame `run` times has always been the first with every Slot's answer. The result is cached inside gpui-component, and its font functions aren't public, so Cairn can't run the listing on another thread either.
- **Not fixed yet.** The fix belongs in gpui-component: check the few families it needs by resolving each, and compare the resolved family's name, instead of listing them all. Until a release has it, the options are a `[patch.crates-io]` to a fork pinned to a commit, which this file would then list, or waiting. Re-check this on any gpui-component upgrade with `cargo run --release -p cairn-budget -- trace --only launch`: the "GPUI app launched" to "window created" step holds it.

## After a `gpui-pre` upgrade: the macOS terminate guard

`crates/app/src/terminate.rs` adds `applicationShouldTerminate:` to GPUI's private `GPUIApplicationDelegate` class, and GPUI's test platform can't reach it. Check by hand that an app-level quit still takes the close flow: run `RUST_LOG=info target/debug/cairn <repo>`, then send it a quit as the Dock does:

```bash
osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier(<pid>).terminate"
```

It passes if the trace shows `kind="quit"` before the app exits. An app that exits with no such line has lost the guard, and an error at launch means the class has gone.
