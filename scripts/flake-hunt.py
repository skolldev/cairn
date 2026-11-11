#!/usr/bin/env python3
"""Runs the test suite again and again, unattended, and reports which tests ever failed.

For flakes that show up in CI and not locally. See docs/flake-hunting.md. Run from the repository
root.

    scripts/flake-hunt.py [--for DURATION] [--iterations N] [--filter NAME] [--until-fail]
                          [--jobs N] [--timeout SECONDS] [-- CARGO TEST ARGUMENTS]

--for hunts that long (90s, 30m, 8h or 1h30m; default 1h unless --iterations is given), and
--iterations stops after that many. --filter hunts only the tests whose names contain NAME, and
--until-fail stops at the first failure. --jobs runs N iterations side by side, for load like a
busy CI runner's. --timeout is how long one test binary may run before it counts as hung
(default 120).

With no cargo arguments it runs what CI's default suite runs: the workspace's tests, then
cairn-app's with its `measure` feature. Cargo arguments replace that, as with git-matrix.sh.
It builds once into target/flake/build, its own target directory, so cargo in the same worktree
can't change the binaries midway. Iterations alternate the oldest and the newest git (CAIRN_GIT_VERSIONS
overrides the list), built by oldest-git.sh.

The report goes to target/flake/<timestamp>/: summary.txt, and a log per failed run in logs/.

Exit status: 0 if nothing failed, hung or leaked, 1 if something did, 2 if the hunt couldn't start
or broke partway.
"""

import argparse
import collections
import datetime
import json
import math
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
# Named, to tell apart the binaries of one crate built twice.
DEFAULT_BUILDS = [("", ["--workspace"]), ("measure", ["-p", "cairn-app", "--features", "measure"])]


def error(message):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(2)


def oldest_git():
    """oldest-git.sh's default version, the one check-git-floor.sh holds to GitVersion::MINIMUM."""
    with open(os.path.join(HERE, "oldest-git.sh")) as f:
        found = re.search(r'^version="\$\{1:-(\d+\.\d+\.\d+)\}"$', f.read(), re.M)
    if not found:
        error("can't read the default version from oldest-git.sh")
    return found.group(1)


def git_versions():
    """The gits to alternate: CAIRN_GIT_VERSIONS, or the oldest supported and the newest released."""
    listed = os.environ.get("CAIRN_GIT_VERSIONS")
    if listed:
        return listed.split()
    tags = subprocess.run(
        ["git", "ls-remote", "--tags", "--refs", "https://github.com/git/git", "v[0-9]*"],
        capture_output=True, text=True, check=True,
    ).stdout
    released = [m.group(1) for m in re.finditer(r"refs/tags/v(\d+\.\d+\.\d+)$", tags, re.M)]
    newest = max(released, key=lambda v: tuple(int(p) for p in v.split(".")))
    return [oldest_git(), newest]


def git_bin(version):
    """The bin directory of that git, built if needed."""
    built = subprocess.run([os.path.join(HERE, "oldest-git.sh"), version], stdout=subprocess.PIPE, text=True)
    if built.returncode != 0:
        error(f"couldn't build git {version}")
    return built.stdout.strip()


class Binary:
    """A test binary, and the tests in it that the hunt runs."""

    def __init__(self, name, path, cwd, test_filter):
        self.name = name
        self.path = path
        self.cwd = cwd
        self.args = [test_filter] if test_filter else []
        listed = subprocess.run([path, "--list", "--format", "terse", *self.args], cwd=cwd,
                                capture_output=True, text=True)
        self.tests = [line[: -len(": test")] for line in listed.stdout.splitlines() if line.endswith(": test")]


def build(builds, build_dir, test_filter):
    """Builds each named set of tests into build_dir, and returns the binaries that have tests to run.

    The binaries run where cargo put them, never copied: tests compare paths against the
    CARGO_BIN_EXE_* paths compiled into them, and cairn-write's runner finds cairn-askpass one
    directory above the test binary. build_dir is the hunt's own, so a `cargo test` in the worktree
    can't change them."""
    binaries = []
    env = dict(os.environ, CARGO_TARGET_DIR=build_dir)
    for name, args in builds:
        cmd = ["cargo", "test", "--locked", "--no-run", "--message-format=json-render-diagnostics", *args]
        print("building: " + " ".join(cmd), flush=True)
        out = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, text=True)
        if out.returncode != 0:
            error("the build failed")
        for line in out.stdout.splitlines():
            msg = json.loads(line)
            if msg.get("reason") != "compiler-artifact" or not msg.get("executable") or not msg["profile"]["test"]:
                continue
            crate_dir = os.path.dirname(msg["manifest_path"])
            label = msg["target"]["name"]
            if os.path.abspath(crate_dir) != os.getcwd():
                label = f"{os.path.basename(crate_dir)}/{label}"
            if name:
                label += f" ({name})"
            binary = Binary(label, msg["executable"], crate_dir, test_filter)
            if binary.tests:
                binaries.append(binary)
    if not binaries:
        error("no test matches" if test_filter else "the build produced no test binaries")
    return binaries


def lock(path):
    """Holds a lock on path until this process exits, or fails if another hunt holds it."""
    import fcntl  # here, so that Windows reaches main()'s refusal rather than an ImportError

    held = open(path, "w")
    try:
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        error(f"another hunt is running in this worktree, and would have its binaries rebuilt ({path})")
    return held


RESULT = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)", re.M)
SECTION = re.compile(r"^---- (\S+) stdout ----$", re.M)
PANIC = re.compile(r"^thread '[^']*'(?: \(\d+\))? panicked at ([^\n]+?):\d+:\n(.*?)(?:\n\n|\nnote: |\nstack backtrace:|\Z)", re.M | re.S)


def failures(output):
    """Each failed test with where it panicked and its message, from libtest's output."""
    failed = [name for name, result in RESULT.findall(output) if result == "FAILED"]
    sections = {}
    starts = list(SECTION.finditer(output))
    for i, m in enumerate(starts):
        end = starts[i + 1].start() if i + 1 < len(starts) else len(output)
        sections[m.group(1)] = output[m.end():end]
    found = []
    for name in failed:
        panics = PANIC.findall(sections.get(name, ""))
        # A test can catch panics on purpose before the one that fails it.
        location, message = panics[-1] if panics else ("(no panic)", "")
        found.append((name, location, message.strip()))
    return found


def one_line(message, limit=200):
    """A message as the summary shows it: on one line, so an assert_eq!'s values stay beside its
    first line, and cut short, so a dump doesn't bury the report. The whole of it is in the log."""
    line = " | ".join(part.strip() for part in message.splitlines() if part.strip())
    return line if len(line) <= limit else line[: limit - 3] + "..."


def session_processes(sid):
    """The commands of the processes still running in a session, by pid."""
    ps = subprocess.run(["ps", "-A", "-o", "pid=,command="], capture_output=True, text=True).stdout
    found = {}
    for line in ps.splitlines():
        pid, _, command = line.strip().partition(" ")
        try:
            if os.getsid(int(pid)) == sid:
                found[int(pid)] = command.strip()
        except OSError:  # gone since ps listed it
            pass
    return found


def kill_session(sid):
    """Kills every process in a session. A git started to be stoppable leads its own process group,
    so a group kill would miss it and what it started, but it stays in the session."""
    for pid in session_processes(sid):
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


def leftovers(sid):
    """What a test binary left running, given a moment to finish, as check-test-processes.sh gives
    it. Killed, so the next run starts clean."""
    for _ in range(5):
        left = session_processes(sid)
        if not left:
            return []
        time.sleep(1)
    kill_session(sid)
    return sorted(set(left.values()))


class Hunt:
    def __init__(self, run_dir, gits, timeout):
        self.run_dir = run_dir
        self.gits = gits
        self.timeout = timeout
        self.iterations = collections.Counter()  # git version -> iterations
        self.runs = collections.Counter()  # (binary, git) -> runs
        self.failed = collections.defaultdict(list)  # (binary, test, location) -> [(git, message, log)]
        self.hung = collections.defaultdict(list)  # binary -> [(git, unfinished tests, log)]
        self.leaked = collections.defaultdict(list)  # binary -> [(git, commands, log)]
        self.slow = collections.defaultdict(list)  # binary -> [(git, seconds, log)]
        self.crashed = collections.defaultdict(list)  # (binary, exit status) -> [(git, log)]
        self.bad_iterations = 0
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.running = set()  # pids of the binaries running now

    def iterate(self, n, binaries):
        """Runs every binary once, on the git whose turn it is, and counts the results unless the
        hunt was stopped meanwhile. Returns whether anything went wrong."""
        version, bin_path = self.gits[n % len(self.gits)]
        env = dict(os.environ, PATH=bin_path + os.pathsep + os.environ["PATH"], CAIRN_EXPECT_GIT_VERSION=version)
        found = []  # (kind, key, entry), each entry starting with the git
        runs = []
        for binary in binaries:
            if self.stop.is_set():
                return False
            env["CARGO_MANIFEST_DIR"] = binary.cwd
            log_name = re.sub(r"[^A-Za-z0-9_.-]+", "-", binary.name).strip("-")
            log = os.path.join(self.run_dir, "logs", f"{n + 1:04d}-git-{version}-{log_name}.log")
            # To a file, not a pipe: a process the test leaves running would hold a pipe open. In its
            # own session, so the processes it starts can be found, and killed, by their session.
            with open(log, "w") as out:
                started = time.monotonic()
                proc = subprocess.Popen([binary.path, *binary.args], cwd=binary.cwd, env=env, stdout=out,
                                        stderr=subprocess.STDOUT, start_new_session=True)
                with self.lock:
                    self.running.add(proc.pid)
                try:
                    proc.wait(timeout=self.timeout)
                    hung = False
                except subprocess.TimeoutExpired:
                    kill_session(proc.pid)
                    proc.wait()
                    hung = True
                finally:
                    with self.lock:
                        self.running.discard(proc.pid)
            took = time.monotonic() - started
            if self.stop.is_set():  # killed by the stop, or finished alongside it: not counted
                kill_session(proc.pid)
                return False
            leaked = [] if hung else leftovers(proc.pid)
            with open(log, errors="replace") as f:
                output = f.read()
            runs.append(binary.name)
            if not hung and took > self.timeout / 2:
                found.append(("slow", binary.name, (version, took, log)))
            if leaked:
                found.append(("leaked", binary.name, (version, leaked, log)))
            if proc.returncode == 0 and not leaked:
                os.remove(log)
                continue
            if hung:
                finished = {name for name, _ in RESULT.findall(output)}
                found.append(("hung", binary.name, (version, [t for t in binary.tests if t not in finished], log)))
            failed = failures(output)
            for test, location, message in failed:
                found.append(("failed", (binary.name, test, location), (version, message, log)))
            if proc.returncode != 0 and not hung and not failed:
                # Died without a test failing: an abort, a signal, a crash in the harness.
                found.append(("crashed", (binary.name, proc.returncode), (version, log)))
        bad = any(kind != "slow" for kind, _, _ in found)
        with self.lock:
            if self.stop.is_set():
                return False
            self.iterations[version] += 1
            for name in runs:
                self.runs[name, version] += 1
            for kind, key, entry in found:
                getattr(self, kind)[key].append(entry)
            self.bad_iterations += bad
        return bad

    def interrupt(self):
        """Stops the hunt, killing every binary still running."""
        self.stop.set()
        with self.lock:
            running = list(self.running)
        for pid in running:
            kill_session(pid)

    def confidence(self):
        """For a targeted test: how far a run of clean iterations can be trusted. With no failure in n
        tries, a failure rate above 3/n would have shown with 95% likelihood (the rule of three)."""
        total = sum(self.iterations.values())
        if not total:
            return []
        if not self.bad_iterations:
            return [f"nothing failed in {total} iterations: its failure rate is likely under "
                    f"{min(100.0, 300 / total):.1f}% (95% confidence)"]
        needed = math.ceil(3 * total / self.bad_iterations)
        return [f"after a fix, about {needed} iterations in a row must pass to show it worked (95% confidence)"]

    def summary(self, header):
        lines = list(header)
        total = sum(self.iterations.values())
        per_git = ", ".join(f"git {v}: {self.iterations[v]}" for v, _ in self.gits)
        lines.insert(0, f"{total} iterations ({per_git})")
        for (binary, test, location), seen in sorted(self.failed.items()):
            lines.append(f"FAILED {binary} {test} at {location}: {self.rate(binary, seen)}")
            # Only a suggestion: the hunt never posts anywhere.
            lines.append(f"  issue: gh issue list --label flaky --search {test}")
            for message, count in collections.Counter(one_line(m) for _, m, _ in seen).most_common():
                lines.append(f"  {count}x {message}")
            lines.extend(self.logs(seen))
        for binary, seen in sorted(self.hung.items()):
            lines.append(f"HUNG {binary}: {self.rate(binary, seen)}")
            for unfinished, count in collections.Counter(", ".join(u) for _, u, _ in seen).most_common():
                lines.append(f"  unfinished: {unfinished}" + (f" ({count}x)" if count < len(seen) else ""))
            lines.extend(self.logs(seen))
        for (binary, status), seen in sorted(self.crashed.items()):
            lines.append(f"CRASHED {binary}, exit {status}: {self.rate(binary, seen)}")
            lines.extend(self.logs(seen))
        for binary, seen in sorted(self.leaked.items()):
            lines.append(f"LEAKED {binary}: {self.rate(binary, seen)}")
            for command in sorted({c for _, commands, _ in seen for c in commands}):
                lines.append(f"  {command}")
            lines.extend(self.logs(seen))
        if not self.found():
            lines.append("nothing failed")
        for binary, seen in sorted(self.slow.items()):
            longest = max(took for _, took, _ in seen)
            lines.append(f"SLOW {binary}: {self.rate(binary, seen)}; longest {longest:.1f}s of {self.timeout:g}s")
        return "\n".join(lines) + "\n"

    def found(self):
        return bool(self.failed or self.hung or self.crashed or self.leaked)

    def rate(self, binary, seen):
        """How many of the binary's runs these are, in all and on each git. Each entry starts with its git."""
        runs = sum(self.runs[binary, v] for v, _ in self.gits)
        by_git = ", ".join(f"git {v}: {sum(1 for s in seen if s[0] == v)} / {self.runs[binary, v]}" for v, _ in self.gits)
        return f"{len(seen)} / {runs} ({100 * len(seen) / runs:.1f}%); {by_git}"

    def logs(self, seen, shown=3):
        logs = sorted({s[-1] for s in seen})
        lines = [f"  log: {log}" for log in logs[:shown]]
        if len(logs) > shown:
            lines.append(f"  and {len(logs) - shown} more in {os.path.join(self.run_dir, 'logs')}")
        return lines


def tree_state():
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.strip()
    return f"commit {commit or 'unknown'}" + (" (uncommitted changes)" if dirty else "")


def duration(text):
    """Seconds, from 90s, 30m, 8h or 1h30m."""
    parts = re.fullmatch(r"(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?", text)
    if not text or not parts:
        raise argparse.ArgumentTypeError(f"not a duration: {text!r} (try 90s, 30m, 8h or 1h30m)")
    h, m, s = (int(p or 0) for p in parts.groups())
    return h * 3600 + m * 60 + s


def raise_interrupt(signum, frame):
    raise KeyboardInterrupt


def keep_awake():
    """Keeps the machine from sleeping until this process exits."""
    pid = str(os.getpid())
    if shutil.which("caffeinate"):
        subprocess.Popen(["caffeinate", "-i", "-w", pid])
    elif shutil.which("systemd-inhibit"):
        subprocess.Popen(["systemd-inhibit", "--what=idle:sleep", "--who=flake-hunt.py", "--why=hunting flaky tests",
                          "tail", f"--pid={pid}", "-f", "/dev/null"], stderr=subprocess.DEVNULL)
    else:
        print("note: neither caffeinate nor systemd-inhibit found, so the machine may sleep midway", flush=True)


def new_run_dir(parent):
    """target/flake/<timestamp>, with a suffix if another hunt started in the same second."""
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    os.makedirs(parent, exist_ok=True)
    for n in range(1, 100):
        run_dir = os.path.join(parent, stamp if n == 1 else f"{stamp}-{n}")
        try:
            os.mkdir(run_dir)
        except FileExistsError:
            continue
        os.mkdir(os.path.join(run_dir, "logs"))
        return run_dir
    error(f"no free run directory for {stamp} under {parent}")


def main():
    if os.name == "nt":
        error("flake-hunt.py runs on macOS and Linux only")
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--for", dest="for_", type=duration, metavar="DURATION",
                        help="hunt this long, as 90s, 30m, 8h or 1h30m (default 1h, unless --iterations)")
    parser.add_argument("--iterations", type=int, help="stop after this many iterations")
    parser.add_argument("--filter", help="hunt only the tests whose names contain this, as libtest filters")
    parser.add_argument("--until-fail", action="store_true", help="stop at the first failure")
    parser.add_argument("--jobs", type=int, default=1, help="iterations to run side by side (default 1)")
    parser.add_argument("--timeout", type=float, default=120, help="seconds a test binary may run (default 120)")
    parser.add_argument("cargo", nargs="*", help="cargo test arguments, after --")
    args = parser.parse_args()
    # Whether or not SIGINT arrived ignored, as a shell's background job gets it. SIGTERM too, so a
    # hunt stopped any other way still writes its report.
    signal.signal(signal.SIGINT, raise_interrupt)
    signal.signal(signal.SIGTERM, raise_interrupt)

    try:
        build_dir = os.path.abspath(os.path.join("target", "flake", "build"))
        os.makedirs(build_dir, exist_ok=True)
        held = lock(os.path.join(build_dir, "hunt.lock"))  # noqa: F841, held until exit
        run_dir = new_run_dir(os.path.abspath(os.path.join("target", "flake")))
        state = tree_state()
        gits = [(v, git_bin(v)) for v in git_versions()]
        binaries = build([("", args.cargo)] if args.cargo else DEFAULT_BUILDS, build_dir, args.filter)
    except KeyboardInterrupt:
        error("interrupted before the hunt started")
    keep_awake()
    tests = sum(len(b.tests) for b in binaries)
    print(f"hunting {tests} test{'s' * (tests != 1)} in {len(binaries)} binar{'ies' if len(binaries) != 1 else 'y'}",
          flush=True)

    hunt = Hunt(run_dir, gits, args.timeout)
    started = time.monotonic()
    # With only --iterations, those decide; with neither, an hour.
    limit = args.for_ if args.for_ is not None else (None if args.iterations is not None else 3600)
    deadline = started + limit if limit is not None else math.inf
    next_n = [0]

    broke = []  # tracebacks of the hunt's own failures, not the tests'

    def job():
        try:
            run_iterations()
        except BaseException:
            # A hunt that broke must not end in a report that says nothing failed.
            broke.append(traceback.format_exc())
            hunt.interrupt()

    def run_iterations():
        """Takes the next iteration and runs it, until the hunt is over."""
        while not hunt.stop.is_set():
            with hunt.lock:
                if args.iterations is not None and next_n[0] >= args.iterations:
                    return
                if time.monotonic() >= deadline:
                    return
                n = next_n[0]
                next_n[0] += 1
            if hunt.iterate(n, binaries) and args.until_fail:
                hunt.stop.set()

    jobs = [threading.Thread(target=job, daemon=True) for _ in range(args.jobs)]
    for j in jobs:
        j.start()
    interrupted = False
    try:
        # A timed join, so Ctrl-C reaches this thread while it waits.
        while any(j.is_alive() for j in jobs):
            for j in jobs:
                j.join(0.2)
    except KeyboardInterrupt:
        interrupted = True
        hunt.interrupt()
        for j in jobs:
            j.join()

    if broke:
        print(broke[0], file=sys.stderr, end="")
        error("the hunt itself broke, so its counts can't be trusted")
    elapsed = round(time.monotonic() - started)
    summary = hunt.summary([state, f"{elapsed}s"] + (["interrupted"] if interrupted else []))
    if args.filter:
        summary += "\n".join(hunt.confidence()) + "\n"
    with open(os.path.join(run_dir, "summary.txt"), "w") as f:
        f.write(summary)
    print(summary + f"report: {run_dir}")
    sys.exit(1 if hunt.found() else 0)


if __name__ == "__main__":
    main()
