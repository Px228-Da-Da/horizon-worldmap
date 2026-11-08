"""Packs mod/ into dist/worldmap-<version>.hnmod (a zip: horizon-native.json and mixins/worldmap/*).

    python tools/pack.py

The version is read from mod/horizon-mixin.toml. The Horizon commit and the
horizon-mixins commit the mod is made for are set below.
"""
import json
import pathlib
import re
import zipfile

HORIZON = "2ebf00a0281c592f79191e76fce0e7d522c6f425"  # Horizon Nightly 2026-11-07
MIXINS = "5a64af938ec86b00d53d16a408aabc0038fb3e57"
HOOKS = 404  # the highest native hook point the mod uses (native_send_chat, API 404)

root = pathlib.Path(__file__).resolve().parent.parent
src = root / "mod"
manifest = (src / "horizon-mixin.toml").read_text(encoding="utf8")
version = re.search(r'^version = "([^"]+)"', manifest, re.M).group(1)
mod_id = re.search(r'^id = "([^"]+)"', manifest, re.M).group(1)

meta = {
    "game": "26.3",
    "hooks": HOOKS,
    "horizon": HORIZON,
    "id": mod_id,
    "mixins": MIXINS,
    "schema": 1,
    "version": version,
}
out = root / "dist" / f"{mod_id}-{version}.hnmod"
out.parent.mkdir(exist_ok=True)
stamp = (2026, 1, 1, 0, 0, 0)  # the same bytes every time
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr(zipfile.ZipInfo("horizon-native.json", stamp), json.dumps(meta, indent=2) + "\n", zipfile.ZIP_DEFLATED)
    for path in sorted(p for p in src.iterdir() if p.suffix in (".rs", ".toml")):
        z.writestr(zipfile.ZipInfo(f"mixins/{mod_id}/{path.name}", stamp), path.read_bytes(), zipfile.ZIP_DEFLATED)
print(out, out.stat().st_size, "bytes")
