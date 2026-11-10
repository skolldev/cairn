# Filesystem events by OS

The watcher (`crates/repo/src/watch.rs`) sees what notify reports, and notify reports what each OS reports. Tests on one OS say nothing about another's events. Check a watcher change against this list, and add every new fact you confirm, with how it was confirmed.

`scripts/linux-tests.sh` runs tests in a Linux container, so inotify behaviour can turn red before CI's Linux job does.

## Facts

notify 8.2.1 throughout.

| OS | Behaviour | Where it shows | Confirmed by |
|---|---|---|---|
| Linux | Opening a file reports `Access(Open)`: inotify watches `OPEN`. Every read of `.git/index`, `config` or a `.gitignore` reports one, the Repo service's own reads included | `classify` ignores `Access` events, and the handler checks for them before rebuilding the excludes, or each read would trigger the next (#32) | `inotify.rs` in notify 8.2.1, the watch mask; `writes_to_an_ignored_untracked_file_refresh_nothing_and_writes_to_an_unignored_one_do` goes red on Linux without that check |
| macOS | Moving a directory out of the worktree reports the directory alone, with nothing for the files inside | a directory with tracked entries below it counts as tracked, so its event is never skipped (#32) | measured; `moving_an_ignored_directory_with_a_tracked_file_out_of_the_worktree_shows_the_deletion` |
| macOS | Each write to an existing file can report `Create(File)` again, then `Modify(Metadata(_))` and `Modify(Data(Content))`: FSEvents reports a path's accumulated flags, not just the latest change. One write costs several refreshes | a test proves a skip by the paths that started External changes (the `external_change` span's `paths`), never by a refresh count under a threshold (#59) | measured by logging the watcher's events |
| macOS | Paths arrive resolved: a temp dir under `/var` arrives as `/private/var` | the watcher matches events against the canonicalized root | `watch::start` |
| Windows | A read walking a directory reports a change to the directory's own timestamps | `classify` drops directory-only `Modify(Any)` and `Modify(Metadata)` events for a path that is still a directory | `watch::classify` |
| Windows | A burst that overflows the change buffer arrives as one rescan, `need_rescan()` | a rescan refreshes everything; tests pause between writes so each one is reported | `a_burst_shorter_than_a_read_costs_at_most_two_status_refreshes` |
