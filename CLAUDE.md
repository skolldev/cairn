## Agent skills

### Issue tracker

Before any `gh` call or a ticket's first commit, read `docs/agents/issue-tracker.md`: issues live in GitHub Issues for skolldev/cairn, with a branch per ticket.

### Triage labels

Default label vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Coding standards

Review rules, including how tests are written: `CODING_STANDARDS.md`. Anything that runs git must pass on the oldest supported git: `scripts/oldest-git.sh` builds it.
