"""Refresh beamng/dotdash.zip, the DotDash build CorsaConnect ships.

    python tools/update_dotdash.py              from ../beamng-dotdash if it's there,
                                                else DotDash's latest GitHub release
    python tools/update_dotdash.py --release    always from the GitHub release

DotDash (github.com/Tudor077/beamng-dotdash) carries CorsaConnect's protocol,
so the launcher installs it rather than a protocol-only mod. This puts its
*public* zip (no private fonts) in beamng/, writes the version beside it for
the launcher to show, and checks the protocol inside is the one in beamng/lua.
Rebuild the exe afterwards: the zip is embedded with include_bytes!.
"""

import io
import json
import os
import subprocess
import sys
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(ROOT, "beamng", "dotdash.zip")
VERSION = os.path.join(ROOT, "beamng", "dotdash.version")
PROTOCOL = "lua/vehicle/protocols/corsaconnect.lua"
SIBLING = os.path.join(os.path.dirname(ROOT), "beamng-dotdash")
RELEASE = "https://github.com/Tudor077/beamng-dotdash/releases/latest/download/dotdash.zip"


def from_source():
    print("building the public zip in", SIBLING)
    subprocess.run([sys.executable, "build.py", "--public", "--zip", "--no-install"], cwd=SIBLING, check=True)
    return open(os.path.join(SIBLING, "dist", "dotdash.zip"), "rb").read()


def from_release():
    print("downloading", RELEASE)
    with urllib.request.urlopen(RELEASE) as r:
        return r.read()


def main():
    use_source = os.path.isdir(SIBLING) and "--release" not in sys.argv
    data = from_source() if use_source else from_release()

    z = zipfile.ZipFile(io.BytesIO(data))
    names = z.namelist()
    if "ui/modules/apps/DotDash/app.json" not in names:
        sys.exit("that zip isn't DotDash")
    if PROTOCOL not in names:
        sys.exit("that DotDash has no CorsaConnect protocol - it needs to be 1.1 or later")
    if any(n.endswith((".woff2", ".otf", ".ttf")) for n in names):
        sys.exit("that zip has font files in it - only the public build may be shipped")
    ours = open(os.path.join(ROOT, "beamng", *PROTOCOL.split("/")), "rb").read().replace(b"\r\n", b"\n")
    if z.read(PROTOCOL).replace(b"\r\n", b"\n") != ours:
        sys.exit("DotDash's copy of the protocol differs from beamng/lua - sync them first")
    version = json.loads(z.read("info.json"))["version"]

    open(OUT, "wb").write(data)
    open(VERSION, "w", encoding="utf-8").write(version + "\n")
    print(f"beamng/dotdash.zip: DotDash {version}, {len(data) // 1024} KB, {len(names)} files")


if __name__ == "__main__":
    main()
