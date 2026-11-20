"""Draws ClipVault.ico: an indigo tile with a clipboard and a stacked card behind it (history).

Every size is drawn on its own (supersampled, then downscaled) so small sizes stay crisp:
16 and 24 px drop the back card and use fewer, bolder lines.

    python assets/make_icon.py        (needs Pillow)
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SS = 8  # supersampling factor

TOP = (122, 146, 255)     # gradient, top-left
BOTTOM = (52, 74, 232)    # gradient, bottom-right
INK = (61, 90, 254)       # lines on the clipboard (app accent)
CLIP = (30, 42, 150)      # clip at the top of the board


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def rrect(draw, box, r, fill):
    draw.rounded_rectangle([round(v) for v in box], radius=round(r), fill=fill)


def draw(size):
    n = size * SS
    u = lambda v: v * n  # fraction of the canvas -> pixels
    small = size <= 24

    # Tile: rounded square with a diagonal gradient.
    inset = 0.03 if small else 0.05
    grad = Image.new("RGB", (n, n))
    gp = grad.load()
    for y in range(n):
        for x in range(n):
            gp[x, y] = lerp(TOP, BOTTOM, (x + y) / (2 * n))
    mask = Image.new("L", (n, n), 0)
    rrect(ImageDraw.Draw(mask), (u(inset), u(inset), u(1 - inset), u(1 - inset)), u(0.22), 255)
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    # Soft light on the upper half of the tile.
    if not small:
        glow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        ImageDraw.Draw(glow).ellipse([u(-0.2), u(-0.65), u(1.2), u(0.5)], fill=(255, 255, 255, 34))
        glow = glow.filter(ImageFilter.GaussianBlur(u(0.04)))
        clipped = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        clipped.paste(glow, (0, 0), Image.composite(glow.getchannel("A"), Image.new("L", (n, n), 0), mask))
        img = Image.alpha_composite(img, clipped)

    # Board geometry.
    if small:
        bx0, by0, bx1, by1, br = 0.22, 0.24, 0.78, 0.86, 0.09
    else:
        bx0, by0, bx1, by1, br = 0.25, 0.27, 0.70, 0.81, 0.065

    # Back card (the "history" behind the current clip).
    if not small:
        back = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        rrect(ImageDraw.Draw(back), (u(bx0 + 0.09), u(by0 - 0.05), u(bx1 + 0.09), u(by1 - 0.05)), u(br),
              (255, 255, 255, 92))
        img = Image.alpha_composite(img, back)

    # Shadow under the board.
    sh = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    off = 0.012 if small else 0.02
    rrect(ImageDraw.Draw(sh), (u(bx0), u(by0 + off), u(bx1), u(by1 + off)), u(br), (12, 18, 90, 120))
    img = Image.alpha_composite(img, sh.filter(ImageFilter.GaussianBlur(u(0.015 if small else 0.03))))

    d = ImageDraw.Draw(img)
    rrect(d, (u(bx0), u(by0), u(bx1), u(by1)), u(br), (255, 255, 255, 255))

    # Clip: a dark pill straddling the top edge of the board.
    cx = (bx0 + bx1) / 2
    cw, ch = (0.30, 0.13) if small else (0.20, 0.10)
    rrect(d, (u(cx - cw / 2), u(by0 - ch / 2), u(cx + cw / 2), u(by0 + ch / 2)), u(ch / 2), CLIP + (255,))
    if not small:  # small highlight on the clip
        rrect(d, (u(cx - cw / 2 + 0.03), u(by0 - ch / 2 + 0.022), u(cx + cw / 2 - 0.03), u(by0 - ch / 2 + 0.042)),
              u(0.01), lerp(CLIP, (255, 255, 255), 0.3) + (255,))

    # Text lines on the board.
    lx0 = bx0 + (0.11 if small else 0.075)
    lx1 = bx1 - (0.11 if small else 0.075)
    if small:
        lines = [(0.46, 1.0), (0.64, 0.62)]
        lh = 0.085
    else:
        lines = [(0.44, 1.0), (0.555, 1.0), (0.67, 0.6)]
        lh = 0.05
    for y, frac in lines:
        rrect(d, (u(lx0), u(y), u(lx0 + (lx1 - lx0) * frac), u(y + lh)), u(lh / 2), INK + (255,))

    return img.resize((size, size), Image.LANCZOS)


def main():
    here = Path(__file__).parent
    images = [draw(s) for s in SIZES]
    images[-1].save(here / "ClipVault.png")
    images[-1].save(here / "ClipVault.ico", format="ICO", sizes=[(s, s) for s in SIZES],
                    append_images=images[:-1])
    print("wrote", here / "ClipVault.ico")


if __name__ == "__main__":
    main()
