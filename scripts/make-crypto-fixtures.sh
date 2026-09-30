#!/usr/bin/env bash
# Regenerate the encrypted-PDF fixtures used by mangle-crypto's tests and by
# `cargo xtask fixtures`.
#
# The encrypted files are produced by **qpdf**, an independent implementation, because a
# fixture we generate ourselves can only prove that we agree with ourselves. qpdf is an
# oracle here: tests only, never product code (GOAL.md 2.3).
#
# Usage: scripts/make-crypto-fixtures.sh [output-directory]
# Requires: qpdf. If it is missing, the tests that use these values still run against
# the hard-coded dictionaries in crates/mangle-crypto/src/handler.rs; this script only
# documents where they came from and lets them be refreshed after a qpdf upgrade.

set -euo pipefail

OUT="${1:-/tmp/opencode}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$OUT"

if ! command -v qpdf >/dev/null; then
  echo "qpdf is not installed; install it with: sudo pacman -S qpdf" >&2
  exit 1
fi

python3 - "$OUT/plain.pdf" <<'PY'
import sys
objs = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R "
    b"/Resources << /Font << /F1 5 0 R >> >> >>",
    None,
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
]
content = b"BT /F1 24 Tf 20 100 Td (Hello world) Tj ET"
objs[3] = b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content), content)

out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
offsets = []
for i, o in enumerate(objs, start=1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for off in offsets:
    out += b"%010d 00000 n \n" % off
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
    len(objs) + 1, xref)
open(sys.argv[1], "wb").write(bytes(out))
PY

qpdf --check "$OUT/plain.pdf" >/dev/null

qpdf --allow-weak-crypto --encrypt user owner 40 -- "$OUT/plain.pdf" "$OUT/enc_r2.pdf"
qpdf --allow-weak-crypto --encrypt user owner 128 --use-aes=n --force-V4 -- \
  "$OUT/plain.pdf" "$OUT/enc_r3.pdf"
qpdf --encrypt user owner 128 --use-aes=y --force-V4 -- "$OUT/plain.pdf" "$OUT/enc_r4a.pdf"
qpdf --encrypt user owner 256 --force-R5 --allow-insecure -- "$OUT/plain.pdf" "$OUT/enc_r5.pdf"
qpdf --encrypt user owner 256 -- "$OUT/plain.pdf" "$OUT/enc_r6.pdf"
qpdf --encrypt "" "" 256 --allow-insecure -- "$OUT/plain.pdf" "$OUT/enc_r6_empty.pdf"

echo "Wrote encrypted fixtures to $OUT:"
for f in enc_r2 enc_r3 enc_r4a enc_r5 enc_r6 enc_r6_empty; do
  printf '  %-16s ' "$f"
  qpdf --show-encryption --password=user "$OUT/$f.pdf" 2>&1 | sed -n '1,2p' | tr '\n' ' '
  echo
done
echo
echo "The /Encrypt dictionaries these produce are hard-coded in"
echo "crates/mangle-crypto/src/handler.rs. After regenerating, update them and re-run:"
echo "  cargo test -p mangle-crypto"
