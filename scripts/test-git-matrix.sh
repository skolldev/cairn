#!/usr/bin/env bash
# Shows that git-matrix.sh closes with one total per version, that its exit code follows cargo's
# through the pipe that collects those totals, and that it prints only what needs reading while
# its logs keep everything. Stands in for cargo and for each built git.
set -euo pipefail
matrix="$(cd "$(dirname "$0")" && pwd)/git-matrix.sh"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT
mkdir -p "$work/bin"

# oldest-git.sh skips its build when ~/git-<version> already holds that version, built with curl.
for version in 9.9.8 9.9.9; do
  mkdir -p "$work/git-$version/bin" "$work/git-$version/libexec/git-core"
  printf '#!/bin/sh\necho "git version %s"\n' "$version" >"$work/git-$version/bin/git"
  : >"$work/git-$version/libexec/git-core/git-remote-http"
  chmod +x "$work/git-$version/bin/git" "$work/git-$version/libexec/git-core/git-remote-http"
done

cat >"$work/bin/cargo" <<'EOF'
#!/bin/sh
case "$CAIRN_EXPECT_GIT_VERSION:$STUB_CARGO" in
  *:no-build)
    echo "error: could not compile" >&2
    exit 101 ;;
  9.9.8:*)
    echo "   Compiling cairn-repo v0.0.0"
    echo "running 3 tests"
    echo "test a_quiet_test ... ok"
    echo ""
    echo "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s"
    echo "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s" ;;
  9.9.9:one-fails)
    echo "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s"
    echo "test a_loud_test ... FAILED"
    echo ""
    echo "failures:"
    echo "---- a_loud_test stdout ----"
    echo "assertion failed: the reason"
    echo "test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s"
    exit 101 ;;
  9.9.9:*)
    echo "test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s" ;;
esac
EOF
chmod +x "$work/bin/cargo"

run_matrix() {
  set +e
  out="$(HOME="$work" TMPDIR="$work" PATH="$work/bin:$PATH" CAIRN_GIT_VERSIONS="9.9.8 9.9.9" STUB_CARGO="$1" "$matrix" 2>&1)"
  status=$?
  set -e
}

run_matrix all-pass
[ "$status" -eq 0 ] || fail "expected exit 0 when all pass, got $status: $out"
grep -qx "git 9.9.8: 7 passed, 0 failed" <<<"$out" || fail "no total for 9.9.8: $out"
grep -qx "git 9.9.9: 10 passed, 0 failed" <<<"$out" || fail "no total for 9.9.9: $out"
for noise in "Compiling" "running 3 tests" "a_quiet_test" "test result: ok"; do
  ! grep -q "$noise" <<<"$out" || fail "printed a passing run's '$noise': $out"
done
log="$(sed -n 's/^log for git 9.9.8: //p' <<<"$out")"
grep -q "test a_quiet_test ... ok" "$log" 2>/dev/null || fail "the log for 9.9.8 lost cargo's output: $out"

run_matrix one-fails
[ "$status" -eq 1 ] || fail "expected exit 1 with a failing version, got $status: $out"
grep -qx "git 9.9.8: 7 passed, 0 failed" <<<"$out" || fail "lost the passing total: $out"
grep -qx "git 9.9.9: 4 passed, 2 failed" <<<"$out" || fail "no failing total: $out"
for loud in "test a_loud_test ... FAILED" "failures:" "assertion failed: the reason" "test result: FAILED"; do
  grep -qF "$loud" <<<"$out" || fail "hid the failure's '$loud': $out"
done

# Nothing failed by count here, so only the note tells this from a clean run of no tests.
run_matrix no-build
[ "$status" -eq 1 ] || fail "expected exit 1 when nothing builds, got $status: $out"
grep -qx "git 9.9.8: 0 passed, 0 failed, cargo failed" <<<"$out" || fail "didn't flag cargo: $out"
grep -qx "error: could not compile" <<<"$out" || fail "hid the compile error: $out"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: git-matrix.sh totals each version, keeps cargo's exit code, and prints only what needs reading"
