#!/usr/bin/env bash
# Runs the checks before a commit; exits non-zero on any failure.
#
# Only the side that changed is checked (Rust: crates/, src-tauri/, Cargo files; frontend: apps/desktop/src,
# packages/, package files). A side that did not change cannot break, so nothing is lost. With a clean tree, or
# with --all, everything is checked. Rust and frontend checks run in parallel.
set -euo pipefail
cd "$(dirname "$0")/.."

changed=$(git status --porcelain --untracked-files=all -- . | cut -c4-)
rust=false
front=false
if [[ "${1:-}" == "--all" || -z "$changed" ]]; then
  rust=true
  front=true
fi
grep -qE '(^|/)(crates/|src-tauri/|Cargo\.(toml|lock)|\.cargo/)' <<<"$changed" && rust=true
grep -qE '(^|/)(apps/(desktop|mobile)/(src/|index\.html|vite|svelte|tsconfig|package)|packages/|package(-lock)?\.json)' <<<"$changed" && front=true
if ! $rust && ! $front; then
  echo "Nothing to check (no code changes)."
  exit 0
fi

logs=$(mktemp -d)
# errexit does not apply inside functions called with || (bash rule), so every step reports its own failure.
check_rust() {
  echo "== clippy"
  cargo clippy --workspace --all-targets --quiet -- -D warnings || return 1
  echo "== rust tests"
  if ! cargo test --workspace --quiet >"$logs/rust-tests.txt" 2>&1; then
    grep -E "FAILED|panicked|error(\[|:)" "$logs/rust-tests.txt" | head -40
    return 1
  fi
  awk '/test result/ { passed += $4; failed += $6 } END { print "rust tests: " passed " passed, " failed " failed" }' "$logs/rust-tests.txt"
}
check_front() {
  local ok=0
  echo "== svelte-check"
  for app in desktop mobile; do
    if ! (cd "apps/$app" && npx svelte-check --tsconfig ./tsconfig.json --fail-on-warnings --output machine >"$logs/svelte-$app.txt" 2>&1); then
      grep -E " (ERROR|WARNING) " "$logs/svelte-$app.txt" | head -20
      ok=1
    fi
    echo "$app: $(tail -1 "$logs/svelte-$app.txt" | cut -d' ' -f2-)"
  done
  echo "== ui tests + frontend build"
  (cd packages/ui && npx vitest run --silent >/dev/null) &
  local ui=$!
  (cd apps/desktop && npx vitest run --silent >/dev/null) &
  local app=$!
  # Both builds also feed the Tauri crates, which embed dist/ when they compile.
  (cd apps/desktop && npx vite build --logLevel error >/dev/null) || ok=1
  (cd apps/mobile && npx vite build --logLevel error >/dev/null) || ok=1
  wait "$ui" || { echo "packages/ui tests FAILED"; ok=1; }
  wait "$app" || { echo "apps/desktop tests FAILED"; ok=1; }
  return $ok
}

status=0
if $rust; then
  check_rust >"$logs/rust.txt" 2>&1 &
  rust_pid=$!
fi
if $front; then
  check_front >"$logs/front.txt" 2>&1 || status=1
  cat "$logs/front.txt"
fi
if $rust; then
  wait "$rust_pid" || status=1
  cat "$logs/rust.txt"
fi
rm -rf "$logs"
if [[ $status -ne 0 ]]; then
  echo "CHECKS FAILED"
  exit 1
fi
echo "ALL CHECKS PASSED ($($rust && echo rust)$($rust && $front && echo ' + ')$($front && echo frontend))"
