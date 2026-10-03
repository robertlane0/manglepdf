#!/usr/bin/env python3
"""Minimal classic-TIFF reader and writer, enough for CCITT fixtures."""
import struct

TYPESIZE = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}
BYTE = {1: 'B', 3: 'H', 4: 'I'}


class Tiff:
    def __init__(self, data):
        self.data = data
        bo = data[:2]
        if bo == b'II':
            self.e = '<'
        elif bo == b'MM':
            self.e = '>'
        else:
            raise ValueError('not a tiff')
        magic, off = struct.unpack_from(self.e + 'HI', data, 2)
        if magic != 42:
            raise ValueError('not a classic tiff')
        self.ifd = off
        self.tags = self._read_ifd(off)

    def _read_ifd(self, off):
        n = struct.unpack_from(self.e + 'H', self.data, off)[0]
        tags = {}
        for i in range(n):
            base = off + 2 + 12 * i
            tag, typ, count = struct.unpack_from(self.e + 'HHI', self.data, base)
            size = TYPESIZE.get(typ, 1) * count
            if size <= 4:
                raw = self.data[base + 8:base + 8 + size]
            else:
                voff = struct.unpack_from(self.e + 'I', self.data, base + 8)[0]
                raw = self.data[voff:voff + size]
            if typ in BYTE:
                vals = list(struct.unpack_from(self.e + BYTE[typ] * count, raw))
            elif typ == 2:
                vals = [raw.split(b'\0')[0].decode('latin-1')]
            else:
                vals = []
            tags[tag] = vals
        return tags

    def get(self, tag, default=None):
        return self.tags.get(tag, default)

    def strips(self):
        """Raw strip bytes in order, concatenated."""
        offs = self.get(273, [])
        counts = self.get(279, [])
        out = b''
        for o, c in zip(offs, counts):
            out += self.data[o:o + c]
        return out

    def bits(self):
        """Unpacked raster as a list of rows of 0/1, 1 = black."""
        w = self.get(256, [0])[0]
        h = self.get(257, [0])[0]
        bps = self.get(258, [1])[0]
        spp = self.get(277, [1])[0]
        photo = self.get(262, [1])[0]
        fill = self.get(266, [1])[0]
        rps = self.get(278, [h])[0] or h
        assert bps == 1 and spp == 1, (bps, spp)
        raw = self.strips()
        stride = (w + 7) // 8
        rows = []
        for y in range(h):
            off = ((y // rps) * rps + (y % rps)) * stride
            rowbits = []
            for x in range(w):
                byte = raw[off + (x >> 3)]
                if fill == 2:
                    bit = (byte >> (x & 7)) & 1
                else:
                    bit = (byte >> (7 - (x & 7))) & 1
                # photometric 0 = min-is-white: a 1 bit is black
                rowbits.append(bit if photo == 0 else 1 - bit)
            rows.append(rowbits)
        return rows


def pack_bits(rows):
    out = bytearray()
    for r in rows:
        acc = 0
        n = 0
        for v in r:
            acc = (acc << 1) | (1 if v else 0)
            n += 1
            if n == 8:
                out.append(acc)
                acc = 0
                n = 0
        if n:
            out.append(acc << (8 - n))
    return bytes(out)


def write_bilevel_tiff(path, columns, rows, photometric=0, fillorder=1):
    """Write raw 1-bit rows as an uncompressed TIFF. `rows` are 0/1 with 1 = black."""
    raw = pack_bits(rows)
    h = len(rows)
    soft = b'MANGLE\x00'
    entries = [
        (256, 3, 1, columns),
        (257, 3, 1, h),
        (258, 3, 1, 1),
        (259, 3, 1, 1),
        (262, 3, 1, photometric),
        (266, 3, 1, fillorder),
        (269, 2, len(soft), 0),
        (273, 4, 1, 0),
        (277, 3, 1, 1),
        (278, 3, 1, h),
        (279, 4, 1, len(raw)),
        (284, 3, 1, 1),
    ]
    n = len(entries)
    ifd_off = 8
    ifd_size = 2 + 12 * n + 4
    soft_off = ifd_off + ifd_size
    data_off = soft_off + len(soft)
    data_off += (4 - data_off % 4) % 4
    body = bytearray()
    for tag, typ, count, value in entries:
        if tag == 269:
            body += struct.pack('<HHII', tag, typ, count, soft_off)
        elif tag == 273:
            body += struct.pack('<HHII', tag, typ, count, data_off)
        elif typ == 4:
            body += struct.pack('<HHII', tag, typ, count, value)
        else:
            assert typ == 3 and count == 1
            body += struct.pack('<HHIHH', tag, typ, count, value, 0)
    blob = bytearray()
    blob += b'II' + struct.pack('<HI', 42, ifd_off)
    blob += struct.pack('<H', n) + body + struct.pack('<I', 0)
    assert len(blob) == soft_off
    blob += soft
    while len(blob) < data_off:
        blob += b'\0'
    blob += raw
    open(path, 'wb').write(bytes(blob))
    return len(raw)


def write_fax_tiff(path, columns, rows, stream, compression=4, two_d=False, photometric=0):
    """A TIFF whose single strip is already a fax stream, for a decoder to read back."""
    raw = stream + b'\0' * ((-len(stream)) % 8)
    h = rows
    soft = b'MANGLE\x00'
    entries = [
        (256, 3, 1, columns),
        (257, 3, 1, h),
        (258, 3, 1, 1),
        (259, 3, 1, compression),
        (262, 3, 1, photometric),
        (266, 3, 1, 1),
        (269, 2, len(soft), 0),
        (273, 4, 1, 0),
        (277, 3, 1, 1),
        (278, 3, 1, h),
        (279, 4, 1, len(raw)),
        (284, 3, 1, 1),
    ]
    if compression == 3:
        entries.append((292, 4, 1, 1 if two_d else 0))
    else:
        entries.append((293, 4, 1, 0))
    n = len(entries)
    ifd_off = 8
    soft_off = ifd_off + 2 + 12 * n + 4
    data_off = soft_off + len(soft)
    data_off += (4 - data_off % 4) % 4
    body = bytearray()
    for tag, typ, count, value in entries:
        if tag == 269:
            body += struct.pack('<HHII', tag, typ, count, soft_off)
        elif tag == 273:
            body += struct.pack('<HHII', tag, typ, count, data_off)
        elif typ == 4:
            body += struct.pack('<HHII', tag, typ, count, value)
        else:
            body += struct.pack('<HHIHH', tag, typ, count, value, 0)
    blob = bytearray()
    blob += b'II' + struct.pack('<HI', 42, ifd_off)
    blob += struct.pack('<H', n) + body + struct.pack('<I', 0)
    blob += soft
    while len(blob) < data_off:
        blob += b'\0'
    blob += raw
    open(path, 'wb').write(bytes(blob))