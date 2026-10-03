#!/usr/bin/env python3
"""Regenerate the CCITT fixtures in crates/mangle-filters/tests/fixtures/ccitt-g4.

Every case goes out and comes back through libtiff: this script writes the raster as an
uncompressed bilevel TIFF, `tiffcp` encodes it, `tiffinfo` confirms the compression is the
one asked for, and `tiffcp -c none` decodes that same stream back to raw samples. What
libtiff produced is frozen into the fixture next to the stream, so the permanent test can
check against it with no libtiff installed and with nothing in this repository having had a
hand in the expectation.

libtiff is an oracle here in the sense GOAL.md 2.3 allows: a test-only tool, never linked,
never invoked by product code, and never needed for the test suite to pass. Read
tests/fixtures/ccitt-g4/README.md before changing what is generated, and in particular
before adding a Group 3 2D case.

    usage: scripts/make-ccitt-fixtures.py [output-directory]
"""
import json
import os
import random
import re
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from ccitt_fixture_tiff import Tiff, pack_bits, write_bilevel_tiff, write_fax_tiff  # noqa: E402

DIR = '/tmp/opencode/ccitt-gt'
REPO = os.path.dirname(HERE)
OUT = (sys.argv[1] if len(sys.argv) > 1
       else os.path.join(REPO, 'crates/mangle-filters/tests/fixtures/ccitt-g4'))


def run(cmd):
    """Run a tool; return what it said on stderr, which is where libtiff complains."""
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode != 0:
        raise SystemExit('{} failed: {}'.format(' '.join(cmd), r.stderr.decode()[:400]))
    return ' '.join(r.stderr.decode().split())


def info(tif):
    return run_out(['tiffinfo', tif])


def run_out(cmd):
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode != 0:
        raise SystemExit('{} failed: {}'.format(' '.join(cmd), r.stderr.decode()[:400]))
    return r.stdout.decode('latin-1')


# ── the rasters ──────────────────────────────────────────────────────────────────

def case_empty():
    w, h = 32, 8
    return w, [[0] * w for _ in range(h)]


def case_all_black():
    w, h = 32, 8
    return w, [[1] * w for _ in range(h)]


def case_one_run():
    w, h = 64, 16
    rows = []
    for y in range(h):
        r = [0] * w
        if 2 <= y < 9:
            for x in range(20, 29):
                r[x] = 1
        rows.append(r)
    return w, rows


def case_alternating():
    """Many alternating runs, each row shifted: vertical, pass and horizontal modes."""
    w, h = 96, 24
    rows = []
    for y in range(h):
        r = [0] * w
        x = 3 + y
        while x < w - 2:
            run = 2 + (x + y) % 5
            for i in range(run):
                if x + i < w:
                    r[x + i] = 1
            x += run + 1 + (y % 3)
        rows.append(r)
    return w, rows


def case_edges():
    """Runs touching both margins, including rows that open and close on black."""
    w, h = 48, 12
    rows = []
    for y in range(h):
        r = [0] * w
        if y % 3 == 0:
            for x in range(0, 5):
                r[x] = 1
        if y % 3 == 1:
            for x in range(w - 7, w):
                r[x] = 1
        if y % 4 == 2:
            for x in range(w):
                r[x] = 1
        rows.append(r)
    return w, rows


def case_shrinking():
    """Rows shorter than the line: content that stops, read against a fuller reference."""
    w, h = 64, 16
    rows = []
    for y in range(h):
        r = [0] * w
        for x in range(max(1, 64 - y * 4)):
            r[x] = 1 if (x // 3 + y) % 2 == 0 else 0
        rows.append(r)
    return w, rows


def case_boundary():
    """Runs of one to six pixels, gaps of one to four: the `b1 - a0 <= 3` rule's edge."""
    w, h = 64, 24
    rows = []
    for y in range(h):
        r = [0] * w
        x = 1 + (y % 4)
        n = 1
        while x < w:
            run = 1 + (n + y) % 6
            for i in range(run):
                if x + i < w:
                    r[x + i] = 1
            x += run + 1 + (n % 4)
            n += 1
        rows.append(r)
    return w, rows


def case_noise(seed=7, w=64, h=64):
    rnd = random.Random(seed)
    return w, [[1 if rnd.random() < 0.35 else 0 for _ in range(w)] for _ in range(h)]


def case_tall(seed=11, w=344, h=287):
    """The shape of a corpus page: a tall strip of text-like noise."""
    rnd = random.Random(seed)
    rows = []
    for y in range(h):
        r = [0] * w
        if y % 7 in (0, 1):
            for x in range(w):
                r[x] = 1 if rnd.random() < 0.08 else 0
        elif y % 7 == 2:
            for x in range(4, w - 4, 3):
                r[x] = 1
        else:
            for x in range(20, 120):
                r[x] = 1 if rnd.random() < 0.15 else 0
        rows.append(r)
    return w, rows


def case_wide():
    """1728 columns: make-up codes, and a run that reaches both margins."""
    w, h = 1728, 8
    rows = []
    for y in range(h):
        r = [0] * w
        if y % 3 == 0:
            for x in range(600, 1100):
                r[x] = 1
        elif y % 3 == 1:
            for x in range(w):
                r[x] = 1
        else:
            for x in range(1720, 1728):
                r[x] = 1
        rows.append(r)
    return w, rows


def case_gradient():
    w, h = 100, 30
    rows = []
    for y in range(h):
        r = [0] * w
        x = 0
        while x < w:
            run = 1 + ((x * 7 + y * 3) % 17)
            for i in range(run):
                if x + i < w:
                    r[x + i] = 1
            x += run + 1 + ((x + y) % 5)
        rows.append(r)
    return w, rows


# (name, raster, libtiff compression).
#
# Group 3 2D is deliberately absent. libtiff is a fine oracle for the other two, and is not
# one for it: its decoder rejects a line built to T.4 ("Line length mismatch at line 0 of
# strip 0 (got 33, expected 32)" for every hand-built first line, including an all-white
# one), and its encoder writes codes that only a decoder which already knows the answer can
# read -- the first element of the first line as a vertical zero, where T.4 requires the
# horizontal pair that encodes the two runs. Its encoder and decoder agree with each other
# and with neither T.4 nor any other implementation, so a fixture frozen from them would
# freeze the disagreement rather than the format.
CASES = [
    ('empty', case_empty, 'g4'),
    ('all-black', case_all_black, 'g4'),
    ('one-run', case_one_run, 'g4'),
    ('alternating', case_alternating, 'g4'),
    ('edges', case_edges, 'g4'),
    ('shrinking', case_shrinking, 'g4'),
    ('boundary', case_boundary, 'g4'),
    ('noise', case_noise, 'g4'),
    ('gradient', case_gradient, 'g4'),
    ('wide', case_wide, 'g4'),
    ('tall', case_tall, 'g4'),
    ('g3-1d-alternating', case_alternating, 'g3:1d'),
    ('g3-1d-boundary', case_boundary, 'g3:1d'),
    ('g3-1d-noise', case_noise, 'g3:1d'),
]


# ── the libtiff pipeline ─────────────────────────────────────────────────────────

def oracle_encode(w, rows, name, compression='g4'):
    """Write a raster, let libtiff encode it, and hand back the stream.

    One strip for the whole image, which is the point: a strip's data is a whole number of
    bytes, so a file with a strip boundary in the middle of the page has its codes padded
    with fill bits there. Concatenating its strips would splice those fill bits into the
    stream a decoder reads, and a PDF has no strips to splice.
    """
    src = os.path.join(DIR, name + '.src.tif')
    enc = os.path.join(DIR, name + '.g4.tif')
    write_bilevel_tiff(src, w, rows, photometric=0)
    run(['tiffcp', '-c', compression, '-r', str(len(rows)), src, enc])
    strips = Tiff(open(enc, 'rb').read()).get(273, [])
    assert len(strips) == 1, '{}: libtiff wrote {} strips'.format(name, len(strips))
    text = info(enc)
    line = [l for l in text.splitlines() if 'Compression Scheme:' in l]
    want = {'g4': 'CCITT Group 4', 'g3:2d': 'CCITT Group 3',
            'g3:1d': 'CCITT Group 3'}[compression]
    if compression.startswith('g3'):
        options = [l for l in text.splitlines() if 'Group 3 Options' in l]
        two_d = options and '2-d encoding' in options[0]
        if (compression == 'g3:2d') != bool(two_d):
            raise SystemExit('{}: {} did not get {}:\n{}'.format(name, compression, want, text))
    if not line or want not in line[0]:
        raise SystemExit('{} was not encoded as {}:\n{}'.format(name, want, text))
    return (Tiff(open(enc, 'rb').read()).strips(),
            line[0].split('Compression Scheme:')[1].strip())


def oracle_decode(name, w, h, stream, compression='g4'):
    """Hand libtiff the very same stream and read back what it says the image is.

    A stream lifted out of a file (a PDF's, say) has to be put back into one, and the
    wrap is a single strip. libtiff complains on stderr about content it could not
    decode and then fills the rest of the page white, so a warning here means this
    oracle is not usable for this case and the fixture must not be built from it.
    """
    wrapped = os.path.join(DIR, name + '.wrap.tif')
    dec = os.path.join(DIR, name + '.raw.tif')
    write_fax_tiff(wrapped, w, h, stream,
                   compression=3 if compression.startswith('g3') else 4,
                   two_d=compression == 'g3:2d', photometric=0)
    text = info(wrapped)
    assert ('Group 3' if compression.startswith('g3') else 'Group 4') in text, text[:400]
    warn = run(['tiffcp', '-c', 'none', wrapped, dec])
    t = Tiff(open(dec, 'rb').read())
    assert t.get(262, [1])[0] == 0, '{}: photometric is not min-is-white'.format(name)
    got = t.bits()
    assert len(got) == h and all(len(r) == w for r in got), name
    return got, warn


def oracle_decode_file(tif, name):
    """libtiff decoding its own file, strip boundaries and all."""
    dec = os.path.join(DIR, name + '.file-out.tif')
    warn = run(['tiffcp', '-c', 'none', tif, dec])
    return Tiff(open(dec, 'rb').read()).bits(), warn


def pdf_ccitt_streams(path):
    """Every CCITTFaxDecode image stream in a PDF, by scanning the raw bytes."""
    data = open(path, 'rb').read()
    out = []
    for m in re.finditer(rb'(\d+)\s+(\d+)\s+obj\b', data):
        end = data.find(b'endobj', m.end())
        body = data[m.end():end if end > 0 else len(data)]
        if b'CCITTFaxDecode' not in body:
            continue
        sm = re.search(rb'stream\r?\n', body)
        if not sm:
            continue
        head = body[:sm.start()]
        out.append({
            'num': int(m.group(1)),
            'width': int(re.search(rb'/Width\s+(\d+)', head).group(1)),
            'height': int(re.search(rb'/Height\s+(\d+)', head).group(1)),
            'k': (lambda v: int(v) if v else None)(re.search(rb'/K\s+(-?\d+)', head).group(1)
                                                         if re.search(rb'/K\s+(-?\d+)', head) else None),
            'stream': body[sm.end():body.rfind(b'endstream')],
        })
    return out


VARIANT = {'g4': 0, 'g3:2d': 1, 'g3:1d': 2}


def emit(name, w, h, stream, raster, note, compression='g4'):
    body = (b'MANGLEG4' + struct.pack('<HHIIII', 1, VARIANT[compression], w, h,
                                      len(stream), len(raster))
            + stream + raster)
    path = os.path.join(OUT, name + '.ccf')
    with open(path, 'wb') as f:
        f.write(body)
    print('{:24} {:5}x{:<4} stream {:6}  raster {:6}  {}'.format(
        name, w, h, len(stream), len(raster), note))


def main():
    os.makedirs(DIR, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    for name, fn, compression in CASES:
        w, rows = fn()
        h = len(rows)
        stream, what = oracle_encode(w, rows, name, compression)
        # Two ways of asking libtiff the same question. They must agree wherever libtiff
        # had nothing to complain about; where it did, only the answer it did not warn
        # about is worth freezing.
        wrapped, w_warn = oracle_decode(name, w, h, stream, compression)
        own, o_warn = oracle_decode_file(os.path.join(DIR, name + '.g4.tif'), name)
        if not w_warn and not o_warn:
            assert wrapped == own, '{}: wrapping the stream changed the answer'.format(name)
        if w_warn or o_warn:
            print('  {}: libtiff warned (wrapped: {} / own file: {}) — taking its own file'
                  .format(name, w_warn or 'clean', o_warn or 'clean'))
        assert not o_warn, '{}: libtiff could not decode its own file: {}'.format(name, o_warn)
        got = wrapped if not w_warn else own
        for y, (a, b) in enumerate(zip(rows, got)):
            assert a == b, '{}: row {} came back as {}\n  wanted {}\n'.format(name, y, b, a)
        emit(name, w, h, stream, pack_bits(got), 'synthetic, ' + what, compression)

    corpus = os.environ.get('CORPUS', os.path.join(REPO, 'corpus', 'wild'))
    for pdf in ('pdfbox__multitiff.pdf', 'pdfjs__issue13372.pdf'):
        path = os.path.join(corpus, pdf)
        if not os.path.exists(path):
            print('skipped: {} is not on disk'.format(path))
            continue
        for im in pdf_ccitt_streams(path)[:1]:
            assert im['k'] == -1 or im['k'] is None, im
            w, h = im['width'], im['height']
            got, warn = oracle_decode('pdf-obj{}'.format(im['num']), w, h, im['stream'])
            assert not warn, '{}: libtiff warned about it: {}'.format(pdf, warn)
            stem = pdf.replace('.pdf', '').replace('__', '-')
            emit('{}-image{}'.format(stem, im['num']), w, h, im['stream'], pack_bits(got),
                 'the CCITT stream out of {}/{}'.format(pdf, im['num']))


if __name__ == '__main__':
    main()