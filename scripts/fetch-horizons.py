#!/usr/bin/env python3
"""Fetch the JPL Horizons check vectors for the registry test.

Usage: python3 scripts/fetch-horizons.py > crates/system-compiler/tests/data/horizons-2026-11-14.toml

For the origin 2026-11-14T00:00:00Z it computes the instant in TDB the same
way gx_core::timescale does (37 s TAI - UTC, 32.184 s TT - TAI, the periodic
TDB - TT term), then asks the Horizons API for the geometric state of every
body center at that Julian date TDB, in ICRF axes, relative to the body's
parent frame: the system barycenter (@0) for the star and the planets, the
third planet's center (@399) for its satellite. It prints the positions in
meters as TOML, one table per frame id, with the request URL in a comment.
This is a one-off data preparation tool: the tests read its output and
never fetch anything.
"""

import datetime
import math
import re
import sys
import urllib.parse
import urllib.request

API = "https://ssd.jpl.nasa.gov/api/horizons.api"
ORIGIN = datetime.datetime(2026, 11, 14, tzinfo=datetime.timezone.utc)
# frame id, Horizons target, Horizons center
TARGETS = [
    (1, "10", "@0"),
    (2, "199", "@0"),
    (3, "299", "@0"),
    (4, "399", "@0"),
    (5, "499", "@0"),
    (6, "599", "@0"),
    (7, "699", "@0"),
    (8, "799", "@0"),
    (9, "899", "@0"),
    (10, "301", "@399"),
]


def tdb_seconds(utc_ms):
    """gx_core::timescale::utc_ms_to_tdb_seconds for instants after 2017."""
    j2000_label_ms = 946_728_000_000
    tt = (utc_ms + 37_000 + 32_184 - j2000_label_ms) / 1000.0
    g = math.radians(357.53 + 0.98560028 * tt / 86400.0)
    return tt + 0.001657 * math.sin(g) + 0.000014 * math.sin(2.0 * g)


def main():
    utc_ms = int(ORIGIN.timestamp() * 1000)
    tdb = tdb_seconds(utc_ms)
    jd = 2451545.0 + tdb / 86400.0
    print("# JPL Horizons geometric states at the origin 2026-11-14T00:00:00Z,")
    print("# fetched with scripts/fetch-horizons.py. Positions in meters, ICRF")
    print("# axes, relative to each frame's parent (km times 1000).")
    print(f"origin_utc_ms = {utc_ms}")
    print(f"tdb_seconds_since_j2000 = {tdb!r}")
    print(f"jd_tdb = {jd!r}")
    for frame, target, center in TARGETS:
        params = {
            "format": "text",
            "COMMAND": f"'{target}'",
            "OBJ_DATA": "NO",
            "MAKE_EPHEM": "YES",
            "EPHEM_TYPE": "VECTORS",
            "CENTER": f"'{center}'",
            "REF_PLANE": "FRAME",
            "OUT_UNITS": "KM-S",
            "VEC_TABLE": "2",
            "TLIST": f"'{jd!r}'",
        }
        url = API + "?" + urllib.parse.urlencode(params)
        text = urllib.request.urlopen(url, timeout=60).read().decode()
        block = text.split("$$SOE")[1].split("$$EOE")[0]
        nums = re.findall(r"([XYZ]) =\s*([-+0-9.E]+)", block)
        xyz = [float(v) * 1000.0 for _, v in nums[:3]]
        print()
        print(f"[frame.{frame}]")
        print(f"# {url}")
        print(f"position_m = [{xyz[0]!r}, {xyz[1]!r}, {xyz[2]!r}]")


if __name__ == "__main__":
    sys.exit(main())
