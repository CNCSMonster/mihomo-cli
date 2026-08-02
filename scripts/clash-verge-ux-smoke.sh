#!/usr/bin/env bash
set -euo pipefail

# Smoke test for docs/design/SPEC-clash-verge-like-ux.md.
#
# Default mode is non-destructive: it validates the CLI binary, help text, and
# read-only commands. Set MIHOMO_CLI_SMOKE_DESTRUCTIVE=1 to run the real
# install/TUN lifecycle on the current machine.

BIN="${MIHOMO_CLI_BIN:-target/debug/mihomo-cli}"
DESTRUCTIVE="${MIHOMO_CLI_SMOKE_DESTRUCTIVE:-0}"

if [[ ! -x "${BIN}" ]]; then
  echo "==> Building ${BIN}"
  cargo build
fi

run() {
  echo "+ $*"
  "$@"
}

assert_help_contains() {
  local cmd="$1"
  local needle="$2"
  if ! "${BIN}" ${cmd} --help | grep -F -- "${needle}" >/dev/null; then
    echo "help for '${cmd}' did not contain: ${needle}" >&2
    exit 1
  fi
}

echo "==> Non-destructive UX checks"
run "${BIN}" --version
assert_help_contains status "Force the system service instance (advanced/debugging)"
assert_help_contains tun "Force the system service instance (advanced/debugging)"
assert_help_contains install 'daily use can run `mihomo-cli tun on`'
run "${BIN}" status || true

echo
if [[ "${DESTRUCTIVE}" != "1" ]]; then
  cat <<'MSG'
Non-destructive checks passed.

To run the real Linux/macOS TUN lifecycle smoke test on a machine where you are
ready to install/remove mihomo services, run:

  MIHOMO_CLI_SMOKE_DESTRUCTIVE=1 scripts/clash-verge-ux-smoke.sh

That mode may prompt for sudo/admin password and changes system services/TUN.
MSG
  exit 0
fi

case "$(uname -s)" in
  Darwin|Linux) ;;
  *) echo "Destructive smoke only supports macOS/Linux for now." >&2; exit 2 ;;
esac

cat <<'MSG'
==> Destructive Clash Verge-like UX smoke
This will exercise the real service lifecycle:
  mihomo-cli uninstall --all
  mihomo-cli tun on
  mihomo-cli status
  mihomo-cli select
  mihomo-cli tun off
It may prompt for confirmation and sudo/admin password.
MSG

if [[ "${MIHOMO_CLI_SMOKE_ASSUME_YES:-0}" != "1" ]]; then
  printf 'Type YES to continue with service install/remove/TUN changes: '
  read -r confirmation
  if [[ "${confirmation}" != "YES" ]]; then
    echo "Destructive smoke cancelled."
    exit 130
  fi
fi

run "${BIN}" uninstall --all || true
run "${BIN}" tun on
run "${BIN}" status
run "${BIN}" select
run "${BIN}" tun off
run "${BIN}" status
