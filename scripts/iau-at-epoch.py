#!/usr/bin/env python3
"""Evaluate IAU rotational elements at J2000 from a NAIF text PCK.

Usage: python3 scripts/iau-at-epoch.py pck00011.tpc

Reads the data blocks of the text kernel, then for each body prints the pole
right ascension, pole declination, prime meridian W0, and rate Wdot at
d = 0 days and T = 0 centuries past J2000 TDB, with every periodic
(nutation precession) term evaluated at its J2000 angle. This is the
computation documented in docs/data-sources.md. It is a one-off data
preparation tool: the compiler never runs it.
"""

import math
import re
import sys

BODIES = [
    ("Sun", 10, None),
    ("Mercury", 199, 1),
    ("Venus", 299, 2),
    ("Earth", 399, 3),
    ("Mars", 499, 4),
    ("Jupiter", 599, 5),
    ("Saturn", 699, 6),
    ("Uranus", 799, 7),
    ("Neptune", 899, 8),
    ("Moon", 301, 3),
]


def read_data(path):
    text = open(path, encoding="latin-1").read()
    blocks = re.findall(r"\\begindata(.*?)(?=\\begintext|\Z)", text, re.S)
    data = {}
    for block in blocks:
        pattern = r"(\w+)\s*=\s*(?:\(([^)]*)\)|([-+0-9.EeDd]+))"
        for name, values, scalar in re.findall(pattern, block):
            nums = [float(v.replace("D", "E")) for v in (values or scalar).split()]
            data[name.upper()] = nums
    return data


def periodic(data, body, system, kind, trig):
    coeffs = data.get(f"BODY{body}_NUT_PREC_{kind}", [])
    if not coeffs:
        return 0.0
    angles = data[f"BODY{system}_NUT_PREC_ANGLES"]
    degree = int(data.get(f"BODY{system}_MAX_PHASE_DEGREE", [1])[0])
    stride = degree + 1
    total = 0.0
    for i, c in enumerate(coeffs):
        if c == 0.0:
            continue
        theta = math.radians(angles[i * stride])  # constant term only at T = 0
        total += c * trig(theta)
    return total


def main():
    data = read_data(sys.argv[1])
    print(f"{'body':8} {'pole_ra_deg':>14} {'pole_dec_deg':>14} {'W0_deg':>14} {'Wdot_deg_day':>16}")
    for name, body, system in BODIES:
        ra = data[f"BODY{body}_POLE_RA"][0]
        dec = data[f"BODY{body}_POLE_DEC"][0]
        pm = data[f"BODY{body}_PM"]
        w0, wdot = pm[0], pm[1]
        if system is not None:
            ra += periodic(data, body, system, "RA", math.sin)
            dec += periodic(data, body, system, "DEC", math.cos)
            w0 += periodic(data, body, system, "PM", math.sin)
        print(f"{name:8} {ra:14.9f} {dec:14.9f} {w0:14.9f} {wdot:16.10f}")


if __name__ == "__main__":
    main()
