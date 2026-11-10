# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

A `scripts/budget.sh run` criterion needs a human at an idle machine (`docs/budget-harness.md`), which an unattended agent never has. Label such an issue `ready-for-human`, or split the Budget run into a `ready-for-human` follow-up and label the rest `ready-for-agent`.

Before applying `ready-for-agent`, check every git command the issue names against `docs/git-versions.md`: argument order, and whether the oldest supported git can do it.

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.
