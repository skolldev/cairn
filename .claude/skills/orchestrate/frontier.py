#!/usr/bin/env python3
"""Print the ready-for-agent tickets the orchestrator may start, oldest first, as JSON lines.

A ticket qualifies when it is open, labelled ready-for-agent and no other state label,
unassigned, has no open sub-issues, and has no open blocker: neither a native GitHub
dependency nor an open issue named on a `Blocked by:` line or under a `## Blocked by`
heading. `--why` also prints the skipped tickets and the reason for each, on stderr.
"""
import json
import re
import subprocess
import sys

STATES = {"needs-triage", "needs-info", "ready-for-human", "wontfix"}


def gh(*args):
    return json.loads(subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout)


def blockers(body):
    refs, in_section = set(), False
    for line in (body or "").splitlines():
        stripped = line.strip()
        if stripped.startswith("#") and not re.match(r"#\d", stripped):
            in_section = bool(re.match(r"#+\s*blocked by", stripped, re.I))
            continue
        if in_section or re.match(r"(\*\*)?blocked by\b", stripped, re.I):
            refs.update(int(n) for n in re.findall(r"#(\d+)", stripped))
    return refs


def main():
    why = "--why" in sys.argv
    repo = gh("repo", "view", "--json", "nameWithOwner")["nameWithOwner"]
    issues = gh("issue", "list", "--state", "open", "--label", "ready-for-agent", "--limit", "200",
                "--json", "number,title,body,labels,assignees")
    open_numbers = {i["number"] for i in gh("issue", "list", "--state", "open", "--limit", "1000", "--json", "number")}
    for issue in sorted(issues, key=lambda i: i["number"]):
        n, reason = issue["number"], None
        labels = {l["name"] for l in issue["labels"]}
        detail = gh("api", f"repos/{repo}/issues/{n}")
        open_subs = (detail.get("sub_issues_summary") or {}).get("total", 0) - (detail.get("sub_issues_summary") or {}).get("completed", 0)
        open_blockers = sorted(b for b in blockers(issue["body"]) if b in open_numbers and b != n)
        if labels & STATES:
            reason = f"conflicting state labels {sorted(labels & STATES)}"
        elif issue["assignees"]:
            reason = "assigned: " + ", ".join(a["login"] for a in issue["assignees"])
        elif open_subs > 0:
            reason = f"parent with {open_subs} open sub-issues"
        elif (detail.get("issue_dependencies_summary") or {}).get("blocked_by", 0) > 0:
            reason = "open native blocker"
        elif open_blockers:
            reason = "blocked by " + ", ".join(f"#{b}" for b in open_blockers)
        if reason:
            if why:
                print(f"skip #{n} {issue['title']}: {reason}", file=sys.stderr)
            continue
        print(json.dumps({"number": n, "title": issue["title"]}))


main()
