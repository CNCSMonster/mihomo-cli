#!/usr/bin/env bash
set -euo pipefail

THRESHOLD="${1:-23}"
OUT_DIR="target/coverage"
HTML_OUT="${OUT_DIR}/tarpaulin-report.html"
XML_OUT="${OUT_DIR}/cobertura.xml"

if ! command -v cargo-tarpaulin >/dev/null 2>&1; then
  cat >&2 <<'EOF'
Error: cargo-tarpaulin is not installed.

Install it with:
  cargo install cargo-tarpaulin

Then run:
  scripts/coverage.sh
EOF
  exit 127
fi

mkdir -p "${OUT_DIR}"

echo "==> Running coverage with minimum threshold: ${THRESHOLD}%"
echo "==> Reports will be written under: ${OUT_DIR}"

cargo tarpaulin \
  --ignore-tests \
  --timeout 120 \
  --fail-under "${THRESHOLD}" \
  --out Html \
  --out Xml \
  --output-dir "${OUT_DIR}"

echo
echo "Coverage check passed."
echo "HTML report: ${HTML_OUT}"
echo "XML report:  ${XML_OUT}"
