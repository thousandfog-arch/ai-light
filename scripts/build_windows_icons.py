from pathlib import Path

from PIL import Image, ImageFilter


ROOT = Path(__file__).resolve().parents[1]
ICONS = ROOT / "src-tauri" / "icons"
MAIN_SOURCE = ICONS / "icon-transparent.png"
TRAY_SOURCE = ICONS / "tray-icon-hd.png"


def fitted_icon(source: Path, size: int, sharpen: bool = False) -> Image.Image:
    image = Image.open(source).convert("RGBA")
    image.thumbnail((size, size), Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(image, ((size - image.width) // 2, (size - image.height) // 2))
    if sharpen:
        canvas = canvas.filter(ImageFilter.UnsharpMask(radius=0.7, percent=120, threshold=2))
    return canvas


def tightly_fitted_icon(source: Path, size: int, sharpen: bool = False) -> Image.Image:
    image = Image.open(source).convert("RGBA")
    bounds = image.getchannel("A").getbbox()
    if bounds:
        image = image.crop(bounds)
    padding = max(1, round(size * 0.04))
    content_size = size - (padding * 2)
    image.thumbnail((content_size, content_size), Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(image, ((size - image.width) // 2, (size - image.height) // 2))
    if sharpen:
        canvas = canvas.filter(ImageFilter.UnsharpMask(radius=0.7, percent=120, threshold=2))
    return canvas


def main() -> None:
    desktop = fitted_icon(MAIN_SOURCE, 512)
    desktop.save(ICONS / "icon.png", optimize=True)

    ico_sizes = [16, 20, 24, 32, 48, 64, 128, 256]
    ico_frames = [fitted_icon(MAIN_SOURCE, size, sharpen=size <= 48) for size in ico_sizes]
    ico_frames[-1].save(
        ICONS / "icon.ico",
        format="ICO",
        append_images=ico_frames[:-1],
        sizes=[(size, size) for size in ico_sizes],
    )

    tightly_fitted_icon(TRAY_SOURCE, 64, sharpen=True).save(
        ICONS / "tray-icon-64.png", optimize=True
    )
    tightly_fitted_icon(TRAY_SOURCE, 256).save(
        ICONS / "tray-icon-256.png", optimize=True
    )


if __name__ == "__main__":
    main()
