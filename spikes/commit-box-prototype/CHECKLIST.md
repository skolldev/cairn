# Real typing checklist, issue #10

From the Floors in #8. German layout. Run once per OS with `cargo run --release`.
Mark each box pass or fail on each OS. Any failure on the Floor list can condemn the
framework, so note what you saw. The right panel shows the escaped value, the
cursor, the marked range and the key log, and the log is also on stderr.

Keep the rate on `Frame`, which is the default. Use `--paste-fix` only for the
second paste pass.

Use real commit messages for the first pass. Most of what follows can be done
inside one.

| Case | macOS | Windows |
| --- | --- | --- |
| **Characters** | | |
| Umlauts and `ß` on the plain layout | Pass | Pass |
| AltGr: `@`, `€`, `{`, `}`, `[`, `]`, `\`, `~` (Windows) | n/a | Pass |
| Option layer: same characters (macOS) | Pass | n/a |
| **Dead keys**, each of `´`, `` ` ``, `^` | | |
| Then a vowel: `é`, `è`, `ê`, and combining marks stays 0 | Pass | Pass |
| Then a space: the bare accent appears | Pass | Pass |
| Then a consonant: accent plus consonant, none lost | Pass | Pass |
| **Commit during a dead key** | | |
| Press `´`, then Cmd-Return or Ctrl-Return. The COMMIT line has no half-composed text and `marked None` | Pass | Pass |
| Press the next key afterwards. No character is lost or doubled | Pass | Pass |
| **macOS only** | | |
| Press-and-hold accents: hold `a`, pick with mouse and with a number | Pass | n/a |
| **Emoji picker** | | |
| Ctrl-Cmd-Space, or Win-. Insert one, then a ZWJ family emoji | Pass | Pass |
| Backspace removes the whole emoji, and the cursor doesn't land inside it | Pass | Pass |
| **Undo and redo** | | |
| Typing, undo, redo restores it | Pass | Pass |
| Compose a dead-key letter, then undo: no broken half-state | Pass | Pass |
| Compose Japanese, commit, undo, redo | Not run | Not run |
| Undo during a composition | Pass | Pass |
| **Selection and navigation** | | |
| Mouse: drag, double-click word, triple-click line | Pass | Pass |
| Shift-arrows, and word and line selection | Pass | Pass |
| Option-arrows or Ctrl-arrows, Home and End, Cmd-arrows | Pass | Pass |
| Copy, cut and paste within the box, with umlauts and emoji | Pass | Pass |
| **Paste** | | |
| `Clipboard: CRLF`, paste: CR count is 0 | FAIL, stock | FAIL, stock |
| `Clipboard: Excel`, paste: CR count is 0 | FAIL, stock | FAIL, stock |
| A real copy from Excel, if you have it: CR count is 0 | Not run | Not run |
| Repeat the three with `--paste-fix` | Pass | Pass |
| **Focus** | | |
| Click the second field and come back: nothing lost | Pass | Pass |
| Cmd-Tab or Alt-Tab away and back: nothing lost | Pass | Pass |
| Same two, during a dead key | Pass | Pass |
| Same two, during a Japanese composition | Not run | Not run |
| **Japanese IME** | | |
| Compose and pick a candidate | Not run | Not run |
| Commit with Return: inserts text, no newline, no COMMIT | Not run | Not run |
| Cancel with Escape: composition gone, text unchanged | Not run | Not run |
| Cmd-Return or Ctrl-Return during a composition: note what happens | Not run | Not run |

Recorded only, can't fail the Floor:

| Case | macOS | Windows |
| --- | --- | --- |
| IME candidates with the window at the screen bottom | Not run | Not run |
| Other IMEs | Not run | Not run |
| Right-to-left text | Not run | Not run |
