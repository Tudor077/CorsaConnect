"""
Generate the CorsaConnect app/launcher icon: a Momo-Prototipo-style steering
wheel (black leather rim, three drilled silver spokes, red center hub).

Two ways to source the artwork:
  * Default: draw it procedurally (no input file needed).
  * If `tools/source_icon.png` exists, use THAT image instead (centered on the
    dark background), so you can drop in the exact icon you want.

Outputs:
  * server/assets/icon.ico + icon_256.png  (Windows launcher window + exe)
  * android/app/src/main/res/mipmap-*/ic_launcher.png (+ _round) at all densities
"""
import math
import os
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "CorsaConnectLOGO.png")
WHITE = (255, 255, 255, 255)

BG = (14, 14, 18, 255)        # app background #0E0E12
RIM = (18, 18, 20, 255)       # black leather rim
RIM_HI = (60, 60, 66, 255)    # subtle rim highlight
SILVER = (150, 150, 156, 255)
SILVER_DK = (96, 96, 102, 255)
RED = (196, 22, 28, 255)
RED_DK = (120, 14, 18, 255)
MONO = (20, 8, 8, 255)        # dark monogram on the red hub


def draw_wheel(S: int) -> Image.Image:
    """Draw the wheel at size S on a transparent canvas (supersampled)."""
    ss = 4
    N = S * ss
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    c = N / 2

    R_out = 0.485 * N
    R_in = 0.405 * N
    Rc = (R_out + R_in) / 2
    rim_w = R_out - R_in

    # --- Spokes (drawn first, tuck under the rim ring) ---
    angles = [90, 210, 330]  # one down, two upper-diagonal
    r0, r1 = 0.12 * N, R_in + rim_w * 0.4
    w0, w1 = 0.052 * N, 0.072 * N
    for deg in angles:
        a = math.radians(deg)
        ux, uy = math.cos(a), math.sin(a)
        px, py = -uy, ux
        poly = [
            (c + ux * r0 + px * w0, c + uy * r0 + py * w0),
            (c + ux * r1 + px * w1, c + uy * r1 + py * w1),
            (c + ux * r1 - px * w1, c + uy * r1 - py * w1),
            (c + ux * r0 - px * w0, c + uy * r0 - py * w0),
        ]
        d.polygon(poly, fill=SILVER)
        # subtle inner shading line
        d.line([(c + ux * r0, c + uy * r0), (c + ux * r1, c + uy * r1)],
               fill=SILVER_DK, width=int(0.006 * N))
        # Row of drilled lightening holes near the rim.
        hr = 0.022 * N
        holes = 5
        rad = 0.30 * N
        spread = w1 * 1.5
        for i in range(holes):
            t = (i - (holes - 1) / 2) / max(1, (holes - 1) / 2)
            hx = c + ux * rad + px * spread * t
            hy = c + uy * rad + py * spread * t
            d.ellipse([hx - hr, hy - hr, hx + hr, hy + hr], fill=(0, 0, 0, 0))

    # --- Rim ring (black) on top of spoke ends ---
    d.ellipse([c - Rc, c - Rc, c + Rc, c + Rc], outline=RIM, width=int(rim_w))
    # thin highlight on the inner edge of the rim
    d.ellipse([c - R_in, c - R_in, c + R_in, c + R_in],
              outline=RIM_HI, width=max(1, int(0.006 * N)))

    # --- Hub: silver mounting disk + red rounded square + monogram ---
    hub_r = 0.155 * N
    d.ellipse([c - hub_r, c - hub_r, c + hub_r, c + hub_r], fill=SILVER_DK)
    d.ellipse([c - hub_r * 0.92, c - hub_r * 0.92, c + hub_r * 0.92, c + hub_r * 0.92],
              fill=SILVER)
    sq = 0.108 * N
    d.rounded_rectangle([c - sq, c - sq, c + sq, c + sq], radius=sq * 0.32, fill=RED)
    d.rounded_rectangle([c - sq, c - sq, c + sq, c + sq * 0.05],
                        radius=sq * 0.32, fill=RED_DK)  # faint top shade
    d.rounded_rectangle([c - sq, c - sq, c + sq, c + sq], radius=sq * 0.32,
                        outline=RED_DK, width=max(1, int(0.004 * N)))
    # Monogram "CC".
    try:
        font = ImageFont.truetype("arialbd.ttf", int(sq * 1.2))
    except OSError:
        try:
            font = ImageFont.truetype("DejaVuSans-Bold.ttf", int(sq * 1.2))
        except OSError:
            font = ImageFont.load_default()
    txt = "CC"
    box = d.textbbox((0, 0), txt, font=font)
    tw, th = box[2] - box[0], box[3] - box[1]
    d.text((c - tw / 2 - box[0], c - th / 2 - box[1]), txt, font=font, fill=MONO)

    return img.resize((S, S), Image.LANCZOS)


def composited(S: int, bg: bool, scale: float = 0.92) -> Image.Image:
    """A square icon: the wheel centered on a rounded background.

    With the real logo (on white) we use a white rounded bg to match it; the
    procedural fallback is designed for the dark app background.
    """
    use_src = os.path.exists(SRC)
    fill = WHITE if use_src else BG
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    if bg:
        m = Image.new("L", (S, S), 0)
        ImageDraw.Draw(m).rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * 0.22), fill=255)
        canvas.paste(Image.new("RGBA", (S, S), fill), (0, 0), m)

    inner = int(S * scale)
    if use_src:
        wheel = Image.open(SRC).convert("RGBA").resize((inner, inner), Image.LANCZOS)
    else:
        wheel = draw_wheel(inner)
    off = (S - inner) // 2
    canvas.alpha_composite(wheel, (off, off))
    return canvas


def main():
    # Windows launcher assets.
    assets = os.path.join(ROOT, "server", "assets")
    os.makedirs(assets, exist_ok=True)
    big = composited(256, bg=True)
    big.save(os.path.join(assets, "icon_256.png"))
    composited(512, bg=True).save(os.path.join(assets, "icon_512.png"))
    big.save(os.path.join(assets, "icon.ico"),
             sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("wrote", assets)

    # Android mipmaps.
    densities = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
    res = os.path.join(ROOT, "android", "app", "src", "main", "res")
    for name, px in densities.items():
        folder = os.path.join(res, f"mipmap-{name}")
        os.makedirs(folder, exist_ok=True)
        square = composited(px, bg=True)
        square.save(os.path.join(folder, "ic_launcher.png"))
        # round variant: same art, circular mask
        m = Image.new("L", (px, px), 0)
        ImageDraw.Draw(m).ellipse([0, 0, px - 1, px - 1], fill=255)
        rnd = Image.new("RGBA", (px, px), (0, 0, 0, 0))
        rnd.paste(square, (0, 0), m)
        rnd.save(os.path.join(folder, "ic_launcher_round.png"))
        print("wrote", folder)


if __name__ == "__main__":
    main()
