#!/usr/bin/env python3
"""Proves a test can fail: breaks the code it guards, requires the test to go red, restores.

For the "Seen red" rule in CODING_STANDARDS.md. Run from the repository root.

One break:
    scripts/seen-red.py FILE OLD NEW -- cargo test -p cairn-repo --test walk some_test

Several, from a JSON array of {"name", "file", "old", "new", "test": [argv...]}:
    scripts/seen-red.py --list breaks.json [--only SUBSTRING]

A break that takes several edits, such as two guards that each keep the test green alone, gives
"edits": [{"file", "old", "new"}, ...] in place of "file", "old" and "new". Its edits are made
together, in order, and may span files.

OLD must occur exactly once in FILE, checked for every edit before any is made. Each test command must be green before its break, and must
run at least one test, so a misspelt filter can't pass as red. A break that doesn't compile
proves nothing, and neither does a run that hangs. The file is restored whatever happens,
Ctrl-C included.

It ends with one summary line: red, still green, unproven (didn't compile, ran no tests, or
hung) and not tried.

Exit status: 0 if every break went red, 1 if any didn't, 2 if a break couldn't be tried.
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
from pathlib import Path

RED, GREEN, NO_COMPILE, NO_TESTS, HUNG = "RED", "STILL-GREEN", "NO-COMPILE", "NO-TESTS", "HUNG"


def run(test, timeout):
    """Runs the test command in its own process group, and classifies how it ended."""
    process = subprocess.Popen(
        test,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        errors="replace",
        start_new_session=True,
    )
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        return HUNG, ""
    if "could not compile" in output:
        return NO_COMPILE, output
    ran = sum(int(n) for n in re.findall(r"^running (\d+) tests?$", output, re.M))
    if ran == 0:
        return NO_TESTS, output
    return (GREEN if process.returncode == 0 else RED), output


def first_failure(output):
    """The line that says why the test failed, for the report. The last panic, because a test
    can catch a panic on purpose before the one that fails it."""
    found = re.findall(r"panicked at [^\n]*\n([^\n]*)", output)
    return found[-1][:100] if found else ""


def restoring(originals):
    """Puts each file back on SIGTERM too: a `finally` alone doesn't run for that."""

    def handler(signum, _frame):
        for path, original in originals.items():
            path.write_bytes(original)
        signal.signal(signum, signal.SIG_DFL)
        os.kill(os.getpid(), signum)

    signal.signal(signal.SIGTERM, handler)


def edits_of(item):
    """The break's edits as (path, old, new): its "edits", or its one "file", "old" and "new"."""
    edits = item.get("edits") or [item]
    return [(Path(e["file"]), e["old"].encode(), e["new"].encode()) for e in edits]


def try_break(item, timeout, known_green):
    """Returns (status, detail) for one break. `known_green` saves re-running a baseline."""
    edits = edits_of(item)
    originals = {}
    for path, _, _ in edits:
        if path in originals:
            continue
        try:
            originals[path] = path.read_bytes()
        except OSError as error:
            return "ERROR", str(error)
    broken = dict(originals)
    for path, old, new in edits:
        count = broken[path].count(old)
        if count != 1:
            return "ERROR", f"OLD occurs {count} times in {path}, not once"
        broken[path] = broken[path].replace(old, new)

    key = tuple(item["test"])
    if key not in known_green:
        status, output = run(item["test"], timeout)
        if status != GREEN:
            return "ERROR", f"before the break the test was {status}: {first_failure(output)}"
        known_green.add(key)

    restoring(originals)
    try:
        for path, content in broken.items():
            path.write_bytes(content)
        status, output = run(item["test"], timeout)
    finally:
        for path, original in originals.items():
            path.write_bytes(original)
    return status, first_failure(output) if status == RED else ""


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--list", help="a JSON array of breaks")
    parser.add_argument("--only", help="only the breaks whose name contains this")
    parser.add_argument("--timeout", type=int, default=300, help="seconds per test run")
    argv = sys.argv[1:]
    test = []
    if "--" in argv:
        split = argv.index("--")
        argv, test = argv[:split], argv[split + 1 :]
    args, positional = parser.parse_known_args(argv)

    if args.list:
        breaks = json.loads(Path(args.list).read_text())
    elif len(positional) == 3 and test:
        file, old, new = positional
        breaks = [{"name": file, "file": file, "old": old, "new": new, "test": test}]
    else:
        parser.error("give FILE OLD NEW -- TEST..., or --list")
    if args.only:
        breaks = [b for b in breaks if args.only in b["name"]]
    if not breaks:
        parser.error("no breaks to try")

    known_green = set()
    counts = {RED: 0, GREEN: 0, "unproven": 0, "ERROR": 0}
    for item in breaks:
        status, detail = try_break(item, args.timeout, known_green)
        print(f"{status:12} {item['name']}  {detail}".rstrip(), flush=True)
        counts[status if status in counts else "unproven"] += 1
    # Last, so a filtered view of the lines above still shows a break that wasn't tried.
    print(
        f"summary: {counts[RED]} red, {counts[GREEN]} still green, "
        f"{counts['unproven']} unproven, {counts['ERROR']} not tried"
    )
    if counts["ERROR"]:
        sys.exit(2)
    sys.exit(0 if counts[RED] == len(breaks) else 1)


if __name__ == "__main__":
    main()
