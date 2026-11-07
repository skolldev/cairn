# Windows checklist (issue #11)

The bench runs itself. These steps are the parts only a human at the Windows machine can do. Use PowerShell from this folder. Record each result in the boxes and paste the log back.

## Before the run

- [ ] Close GitKraken, and keep it closed until the run is done. It writes commit-graph layers and a `multi-pack-index` into the fixtures.
- [ ] Leave Defender in its normal state. Don't add exclusions for the run.
- [ ] Record the fixture state, then don't change it. The bench turns the commit-graph off per walk, so `linux-nograph` isn't used.

```powershell
cd $HOME\cairn-fixtures\linux-graph
Get-Content .git\objects\info\commit-graphs\commit-graph-chain
Get-ChildItem .git\objects\pack | Select-Object Name, Length
git rev-list --count --all
git status --porcelain=v2
```

- [ ] Status shows only the 13 case-colliding baseline paths, all `1 .M` with equal hashes. If anything else shows, stop: the bench refuses to change a worktree that isn't at its baseline.
- [ ] `git --version` is Git for Windows, and `git config --show-origin core.fscache` shows where `fscache` comes from. gix reads the git binary's config, so it sees the same value.

## Run

```powershell
cargo build --release
.\target\release\gix-timings-prototype.exe all | Tee-Object results\windows-full.log
```

- [ ] The run finished, and the last status line reads `Restored: worktree status is back to the baseline.`
- [ ] If it stopped part-way, run `.\target\release\gix-timings-prototype.exe restore`, then check `git status --porcelain=v2` shows only the baseline again.
- [ ] Nothing else heavy was running. Note anything that was.

## What to look at

- [ ] **Walks:** every row has a result, and the notes say `identical order to git`. Any other order note is recorded, not a failure.
- [ ] **Status timings:** full status has a 1 s Floor on Windows. The other Floors are the same as on macOS.
- [ ] **A miss:** the `git worst` column on the same row is the CLI fallback's time. A miss where `git` passes sends that path to the CLI. If `git` misses too, it's a design problem for the Budget rewrite.
- [ ] **Kernel parity:** `PASS`, and the baseline line reads `13 of 13`. A baseline difference is recorded, not a failure.
- [ ] **Filters fixture:** both parity passes and hunk parity read `PASS`. The `upper` clean filter is `tr a-z A-Z` run through `sh`. If gix can't find `sh` or `tr` and errors, record the error. That's a fidelity result, not a bench bug.

## Cold runs, optional and recorded only

Reboot, don't open the fixture, then:

```powershell
.\target\release\gix-timings-prototype.exe walk --cold | Tee-Object results\windows-cold-walk.log
.\target\release\gix-timings-prototype.exe status --cold | Tee-Object results\windows-cold-status.log
```

The second command runs after the first, so its cache isn't fully cold. Record it as such.

## Results

Paste `results\windows-full.log` into the issue, or send it back to fill in the README.

- Date:
- Machine and drive:
- Git for Windows version:
- Fixture state (chain layers, packs, `multi-pack-index`):
- Notes:
