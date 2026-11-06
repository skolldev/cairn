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
| Umlauts and `ß` on the plain layout | | |
| AltGr: `@`, `€`, `{`, `}`, `[`, `]`, `\`, `~` (Windows) | n/a | |
| Option layer: same characters (macOS) | | n/a |
| **Dead keys**, each of `´`, `` ` ``, `^` | | |
| Then a vowel: `é`, `è`, `ê`, and combining marks stays 0 | | |
| Then a space: the bare accent appears | | |
| Then a consonant: accent plus consonant, none lost | | |
| **Commit during a dead key** | | |
| Press `´`, then Cmd-Return or Ctrl-Return. The COMMIT line has no half-composed text and `marked None` | | |
| Press the next key afterwards. No character is lost or doubled | | |
| **macOS only** | | |
| Press-and-hold accents: hold `a`, pick with mouse and with a number | | n/a |
| **Emoji picker** | | |
| Ctrl-Cmd-Space, or Win-. Insert one, then a ZWJ family emoji | | |
| Backspace removes the whole emoji, and the cursor doesn't land inside it | | |
| **Undo and redo** | | |
| Typing, undo, redo restores it | | |
| Compose a dead-key letter, then undo: no broken half-state | | |
| Compose Japanese, commit, undo, redo | | |
| Undo during a composition | | |
| **Selection and navigation** | | |
| Mouse: drag, double-click word, triple-click line | | |
| Shift-arrows, and word and line selection | | |
| Option-arrows or Ctrl-arrows, Home and End, Cmd-arrows | | |
| Copy, cut and paste within the box, with umlauts and emoji | | |
| **Paste** | | |
| `Clipboard: CRLF`, paste: CR count is 0 | FAIL, stock | |
| `Clipboard: Excel`, paste: CR count is 0 | FAIL, stock | |
| A real copy from Excel, if you have it: CR count is 0 | | |
| Repeat the three with `--paste-fix` | | |
| **Focus** | | |
| Click the second field and come back: nothing lost | | |
| Cmd-Tab or Alt-Tab away and back: nothing lost | | |
| Same two, during a dead key | | |
| Same two, during a Japanese composition | | |
| **Japanese IME** | | |
| Compose and pick a candidate | | |
| Commit with Return: inserts text, no newline, no COMMIT | | |
| Cancel with Escape: composition gone, text unchanged | | |
| Cmd-Return or Ctrl-Return during a composition: note what happens | | |

Recorded only, can't fail the Floor:

| Case | macOS | Windows |
| --- | --- | --- |
| IME candidates with the window at the screen bottom | | |
| Other IMEs | | |
| Right-to-left text | | |
