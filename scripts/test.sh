#!/usr/bin/env bash
# esp32sim test runner — no ESP32 hardware, toolchain or network needed.
#
#   scripts/test.sh              fast suites: rust + js + e2e   (~5 min, most of it e2e)
#   scripts/test.sh rust         Rust unit + integration tests (mock firmware, mocked circuit / AI)
#   scripts/test.sh js           JavaScript unit tests of the GUI (node --test + jsdom)
#   scripts/test.sh e2e          Playwright end-to-end GUI tests (builds the release binary)
#   scripts/test.sh scenarios    real firmware images (Arduino, MicroPython, esp-hal) — slower
#   scripts/test.sh desktop      Tauri desktop shell: tests on the mock runtime + build check
#                                (Linux needs the WebKitGTK dev packages, see desktop/README.md)
#   scripts/test.sh all          rust + js + e2e + scenarios
#
# Several suites can be given: scripts/test.sh rust js
set -uo pipefail
cd "$(dirname "$0")/.."

suites=("$@")
[ ${#suites[@]} -eq 0 ] && suites=(rust js e2e)
[ "${suites[0]}" = "all" ] && suites=(rust js e2e scenarios)

results=()
failed=0

step() { printf '\n\033[1;34m=== %s ===\033[0m\n' "$1"; }

run() { # name, command...
  local name=$1; shift
  local t0=$SECONDS
  if "$@"; then
    results+=("✔ $name ($((SECONDS - t0)) s)")
  else
    results+=("✘ $name ($((SECONDS - t0)) s)")
    failed=1
  fi
}

need_node() {
  if ! command -v node >/dev/null; then echo "node (>= 18) is required for the JS and GUI tests"; return 1; fi
  if [ ! -d ui-tests/node_modules ]; then (cd ui-tests && npm ci --no-audit --no-fund) || return 1; fi
}

for s in "${suites[@]}"; do
  case "$s" in
    rust)
      step "Rust: unit + integration tests"
      run "rust" cargo test --release --workspace
      ;;
    js)
      step "JavaScript: GUI unit tests"
      if need_node; then run "js" npm --prefix ui-tests run --silent test:unit; else results+=("✘ js (node missing)"); failed=1; fi
      ;;
    e2e)
      step "GUI end-to-end tests (Playwright + Chromium)"
      if need_node; then
        # needs a Playwright Chromium: run `npx playwright install chromium` in ui-tests/ once
        run "e2e" bash -c 'cargo build --release -p esp32sim && cd ui-tests && npx playwright test'
      else results+=("✘ e2e (node missing)"); failed=1; fi
      ;;
    scenarios)
      step "Real firmware scenarios"
      run "scenarios" cargo test --release -p esp32sim --test scenarios -- --ignored
      ;;
    desktop)
      step "Desktop shell (Tauri)"
      run "desktop tests" cargo test --manifest-path desktop/src-tauri/Cargo.toml --no-default-features --features mock
      run "desktop build" cargo check --manifest-path desktop/src-tauri/Cargo.toml
      ;;
    *)
      echo "unknown suite '$s' (rust, js, e2e, scenarios, desktop, all)"; exit 2;;
  esac
done

printf '\n\033[1m=== summary ===\033[0m\n'
printf '%s\n' "${results[@]}"
exit $failed
