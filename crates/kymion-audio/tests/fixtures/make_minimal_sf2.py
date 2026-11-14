#!/usr/bin/env python3
"""Generate minimal.sf2: one preset (bank 0, preset 0, "Test") with one looped sine sample.

Used by the kymion-audio tests; regenerate with:
    python3 make_minimal_sf2.py minimal.sf2
"""
import math
import struct
import sys


def chunk(tag, data):
    if len(data) % 2:
        data += b"\0"
    return tag + struct.pack("<I", len(data)) + data


def lst(tag, body):
    return chunk(b"LIST", tag + body)


def name(s):
    return s.encode().ljust(20, b"\0")


N = 200  # sample frames
smpl = b"".join(struct.pack("<h", int(12000 * math.sin(2 * math.pi * i / 50))) for i in range(N + 46))

info = lst(
    b"INFO",
    chunk(b"ifil", struct.pack("<HH", 2, 1)) + chunk(b"isng", b"EMU8000\0") + chunk(b"INAM", b"Kymion Test\0"),
)
sdta = lst(b"sdta", chunk(b"smpl", smpl))

phdr = struct.pack("<20sHHHIII", name("Test"), 0, 0, 0, 0, 0, 0) + struct.pack("<20sHHHIII", name("EOP"), 0, 0, 1, 0, 0, 0)
pbag = struct.pack("<HH", 0, 0) + struct.pack("<HH", 1, 0)
pmod = b"\0" * 10
pgen = struct.pack("<HH", 41, 0) + struct.pack("<HH", 0, 0)  # instrument #0, terminal
inst = struct.pack("<20sH", name("Sine"), 0) + struct.pack("<20sH", name("EOI"), 1)
ibag = struct.pack("<HH", 0, 0) + struct.pack("<HH", 2, 0)
imod = b"\0" * 10
# sampleModes=1 (loop), sampleID last
igen = struct.pack("<Hh", 54, 1) + struct.pack("<HH", 53, 0) + struct.pack("<HH", 0, 0)
shdr = struct.pack("<20sIIIIIBbHH", name("sine"), 0, N, 50, 150, 44100, 60, 0, 0, 1) + struct.pack(
    "<20sIIIIIBbHH", name("EOS"), 0, 0, 0, 0, 0, 0, 0, 0, 0
)
pdta = lst(
    b"pdta",
    chunk(b"phdr", phdr) + chunk(b"pbag", pbag) + chunk(b"pmod", pmod) + chunk(b"pgen", pgen)
    + chunk(b"inst", inst) + chunk(b"ibag", ibag) + chunk(b"imod", imod) + chunk(b"igen", igen)
    + chunk(b"shdr", shdr),
)
body = b"sfbk" + info + sdta + pdta
open(sys.argv[1] if len(sys.argv) > 1 else "minimal.sf2", "wb").write(b"RIFF" + struct.pack("<I", len(body)) + body)
