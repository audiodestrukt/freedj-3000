#!/usr/bin/env python3
"""make_splash.py — draw the boot splash (splash.png) in audiodestrukt's brand.

    python3 deploy/pi/autostart/make_splash.py      ->  deploy/pi/autostart/splash.png

The brand as audiodestrukt.com draws it: the transistor mark (its favicon,
an NPN symbol in lime on near-black), the wordmark "audio" in light grey and
"destrukt" in lime, bold Courier New (Liberation Mono Bold has the same
metrics), and OPENDECK small and muted under it.  The image's own ground is
BG; the Plymouth script fills the rest of the screen with the same colour.
"""
import os
from PIL import Image, ImageDraw, ImageFont

BG = (0x0d, 0x0d, 0x0d)
TEXT = (0xe8, 0xe8, 0xe8)
ACCENT = (0xc8, 0xff, 0x00)
MUTED = (0x77, 0x77, 0x77)
SS = 4                      # supersampling for clean edges
W, H = 1600, 520            # final size; the script scales it to the screen

BOLD = ["/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"]
REGULAR = ["/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
           "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"]


def font(paths, size):
    for p in paths:
        if os.path.exists(p):
            return ImageFont.truetype(p, size)
    raise SystemExit(f"none of {paths} found (apt install fonts-liberation)")


def mark(d, x0, y0, s):
    """The favicon's transistor on its 32-unit grid, at (x0, y0), s px/unit."""
    P = lambda x, y: (x0 + x * s, y0 + y * s)
    def line(a, b, w):
        d.line([P(*a), P(*b)], fill=ACCENT, width=round(w * s))
        for p in (a, b):   # round caps
            r = w * s / 2
            cx, cy = P(*p)
            d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=ACCENT)
    line((12, 7), (12, 25), 2.5)      # base bar
    line((2, 16), (12, 16), 2)        # base lead
    line((12, 10), (25, 4), 2)        # collector
    line((25, 2), (25, 6), 2)
    line((12, 22), (25, 28), 2)       # emitter
    line((25, 26), (25, 30), 2)
    d.polygon([P(22.5, 26.6), P(17.8, 26.8), P(19.2, 23.2)], fill=ACCENT)


def main():
    im = Image.new("RGB", (W * SS, H * SS), BG)
    d = ImageDraw.Draw(im)
    big = font(BOLD, 150 * SS)
    small = font(REGULAR, 40 * SS)
    track = 0.05 * 150 * SS            # the site's letter-spacing: 0.05em

    # Wordmark, letter by letter for the tracking.
    word = [(c, TEXT) for c in "audio"] + [(c, ACCENT) for c in "destrukt"]
    adv = big.getlength("a") + track
    word_w = adv * len(word) - track
    mark_s = 150 * SS / 32 * 1.15      # the mark a little taller than the caps
    gap = 50 * SS
    total = 32 * mark_s + gap + word_w
    x = (W * SS - total) / 2
    asc, desc = big.getmetrics()
    cy = H * SS * 0.42
    mark(d, x, cy - 16 * mark_s, mark_s)
    x += 32 * mark_s + gap
    for c, col in word:
        d.text((x, cy), c, font=big, fill=col, anchor="lm")
        x += adv

    # OPENDECK under it, centred under the wordmark, muted and spaced out.
    label, sp = "OPENDECK", 0.35 * 40 * SS
    lw = sum(small.getlength(c) + sp for c in label) - sp
    lx = (W * SS - lw) / 2
    for c in label:
        d.text((lx, cy + 150 * SS), c, font=small, fill=MUTED, anchor="lm")
        lx += small.getlength(c) + sp

    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "splash.png")
    im.resize((W, H), Image.LANCZOS).save(out, optimize=True)
    print(out)


if __name__ == "__main__":
    main()
