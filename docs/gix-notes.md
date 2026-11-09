# gix behaviour at the pinned revision

Cairn pins gix to a revision (`Cargo.toml`). These are the behaviours a caller can't see from its signatures, each checked in gix's source at that revision. Read the source in the cargo checkout (`~/.cargo/git/checkouts/gitoxide-*/<short rev>`), and add every new fact you confirm. When the pin moves, check each fact again.

The rules for using gix on the fast path live with the read code, in `crates/read/src/lib.rs`.

## Facts

| Behaviour | Where it shows | Checked in |
|---|---|---|
| The exclude stack (`Repository::excludes`) reads a directory's `.gitignore` when a lookup first reaches that directory, not when the stack is built. It keeps the patterns of the directories the last lookup passed through, and drops a directory's patterns once a lookup goes elsewhere or names that directory itself | `cairn_read::Excludes`: a `.gitignore` change must rebuild the stack, and every event's path is looked up, a tracked one included, so a replaced directory's old patterns go (#32). A test of a stale pattern has to make a lookup read it first | `gix-worktree/src/stack/state/ignore.rs` (`push_directory`), `gix-fs/src/stack.rs` (`make_relative_path_current`) |
| `Repository::excludes` returns an `AttributeStack` that borrows the repository. `detach()` gives the plumbing `gix_worktree::Stack`, which owns its state and can be kept | `Excludes` holds the detached stack | `gix/src/attribute_stack.rs` |
| A stack lookup takes an object finder, for a `.gitignore` it must read from the index instead of the worktree. `gix::objs::find::Never` makes such a lookup fail instead | `Excludes::skips` passes `Never`, and treats a failed lookup as not excluded | `gix-worktree/src/stack/state/ignore.rs` |
| The stack matches as `core.ignoreCase` says (default false), but the index's `entry_by_path` and `path_is_directory` are case-sensitive; their `_icase` forms need an `AccelerateLookup` that borrows the index | `Excludes` folds the index's paths itself when `core.ignoreCase` is set | `gix/src/repository/attributes.rs`, `gix-index/src/access/mod.rs` |
