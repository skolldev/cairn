## Agent skills

### Issue tracker

Issues live in GitHub Issues for skolldev/cairn, via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Coding standards

Review rules, including how tests are written: `CODING_STANDARDS.md`. Runner changes must pass on git 2.23: `scripts/oldest-git.sh` builds it.
