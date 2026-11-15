#!/usr/bin/env python3
"""Print the IAU rotational elements of every body as data/system.toml lines.

Usage: python3 scripts/iau-elements.py pck00011.tpc

Reads the data blocks of the NAIF text kernel and, for each body, prints the
pole right ascension and declination polynomials (degrees, degrees per Julian
century), the prime meridian polynomial (degrees, degrees per day, degrees
per day squared), and every periodic (nutation precession) term whose
coefficients are not all zero, with its angle polynomial (degrees, degrees
per century, degrees per century squared). The module evaluates them at the
origin (crates/system-compiler/src/rotation.rs). This is a one-off data
preparation tool: the module never runs it.
"""

import re
import sys

BODIES = [
    ("Sun", 10, None),
    ("Mercury", 199, 1),
    ("Venus", 299, None),
    ("Earth", 399, 3),
    ("Mars", 499, 4),
    ("Jupiter", 599, 5),
    ("Saturn", 699, None),
    ("Uranus", 799, None),
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


def fmt(x):
    return repr(float(x))


def main():
    data = read_data(sys.argv[1])
    for name, body, system in BODIES:
        ra = data[f"BODY{body}_POLE_RA"]
        dec = data[f"BODY{body}_POLE_DEC"]
        pm = data[f"BODY{body}_PM"]
        print(f"# {name} (BODY{body})")
        print(f"pole_ra_deg = [{fmt(ra[0])}, {fmt(ra[1])}]")
        print(f"pole_dec_deg = [{fmt(dec[0])}, {fmt(dec[1])}]")
        print(f"prime_meridian_deg = [{fmt(pm[0])}, {fmt(pm[1])}, {fmt(pm[2])}]")
        terms = []
        if system is not None and f"BODY{system}_NUT_PREC_ANGLES" in data:
            angles = data[f"BODY{system}_NUT_PREC_ANGLES"]
            degree = int(data.get(f"BODY{system}_MAX_PHASE_DEGREE", [1])[0])
            stride = degree + 1
            cra = data.get(f"BODY{body}_NUT_PREC_RA", [])
            cdec = data.get(f"BODY{body}_NUT_PREC_DEC", [])
            cpm = data.get(f"BODY{body}_NUT_PREC_PM", [])
            count = max(len(cra), len(cdec), len(cpm))
            for i in range(count):
                a = cra[i] if i < len(cra) else 0.0
                d = cdec[i] if i < len(cdec) else 0.0
                p = cpm[i] if i < len(cpm) else 0.0
                if a == 0.0 and d == 0.0 and p == 0.0:
                    continue
                poly = angles[i * stride:(i + 1) * stride] + [0.0] * (3 - stride)
                terms.append((poly, a, d, p))
        if terms:
            print("rotation_terms = [")
            for poly, a, d, p in terms:
                print(
                    f"  {{ angle_deg = [{fmt(poly[0])}, {fmt(poly[1])}, {fmt(poly[2])}], "
                    f"ra_deg = {fmt(a)}, dec_deg = {fmt(d)}, pm_deg = {fmt(p)} }},"
                )
            print("]")
        print()


if __name__ == "__main__":
    main()
