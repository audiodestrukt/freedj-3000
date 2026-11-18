"""keycap.py — deck5's key cap at any size.

caps.key_cap() draws a cap of params.KEY; the XDJ face has keys of several
sizes (layout.toml cap_mm), so this sets caps' KEY for one call.
"""
import caps


def key_cap(size):
    old = caps.KEY
    caps.KEY = tuple(size)
    try:
        return caps.key_cap()
    finally:
        caps.KEY = old


def sizes(keys):
    """{(w, h): count} over a KEYS list."""
    out = {}
    for *_, s in keys:
        out[s] = out.get(s, 0) + 1
    return out


def stem(size):
    w, h = size
    return f"cap_key_{w:g}x{h:g}"
