"""Generates crates/app/assets/icon.ico, icon.png and icon64.rgba (the window icon) from code.

    python scripts/make_icon.py
"""
from pathlib import Path
from PIL import Image, ImageDraw

SS = 4  # supersampling factor
OUT = Path(__file__).resolve().parent.parent / "crates" / "app" / "assets"


def draw(size: int) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # rounded square with a vertical teal gradient
    grad = Image.new("RGBA", (s, s))
    gd = ImageDraw.Draw(grad)
    top, bottom = (62, 201, 167), (30, 120, 130)
    for y in range(s):
        t = y / (s - 1)
        gd.line([(0, y), (s, y)], fill=tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=int(s * 0.22), fill=255)
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    white = (255, 255, 255, 255)
    shade = (20, 70, 80, 255)
    u = s / 100.0

    # box lid
    d.rounded_rectangle([18 * u, 24 * u, 82 * u, 42 * u], radius=4 * u, fill=white)
    # box body
    d.rounded_rectangle([23 * u, 40 * u, 77 * u, 78 * u], radius=4 * u, fill=white)
    # handle slot
    d.rounded_rectangle([41 * u, 50 * u, 59 * u, 57 * u], radius=3.5 * u, fill=shade)
    # zipper line on the lid
    for i in range(6):
        x = (26 + i * 9.5) * u
        d.rectangle([x, 31 * u, x + 4.5 * u, 35 * u], fill=shade)

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    imgs = [draw(n) for n in sizes]
    imgs[-1].save(OUT / "icon.png")
    imgs[-1].save(OUT / "icon.ico", sizes=[(n, n) for n in sizes], append_images=imgs[:-1])
    (OUT / "icon64.rgba").write_bytes(draw(64).tobytes())
    print("wrote", OUT)


if __name__ == "__main__":
    main()
