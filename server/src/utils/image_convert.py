"""
Image ingestion normalization: convert camera raw files (and other formats
Pillow alone cannot or should not handle) into JPEG bytes for the indexing
pipeline.

Design goals:
- Bytes that are already a supported web image within the size limit pass
  through untouched, so the legacy export/preview flows produce byte-identical
  embeddings (no drift for existing users).
- Raw files are served from their embedded JPEG preview when possible
  (milliseconds via LibRaw) and only fully demosaiced as a last resort.
- rawpy and pillow-heif are optional at runtime: if the import fails (e.g. a
  stripped-down build), conversion reports a clear error and the plugin falls
  back to its export-based flow.
"""

from __future__ import annotations

import io
import os

from PIL import Image, ImageOps

from config import logger

try:
    import rawpy

    _RAWPY_AVAILABLE = True
except Exception:  # pragma: no cover - depends on build environment
    rawpy = None
    _RAWPY_AVAILABLE = False

try:
    from pillow_heif import register_heif_opener

    register_heif_opener()
    _HEIF_AVAILABLE = True
except Exception:  # pragma: no cover - depends on build environment
    _HEIF_AVAILABLE = False


class UnsupportedImageError(Exception):
    """Raised when incoming bytes cannot be converted to a usable image."""


RAW_EXTENSIONS = {
    ".3fr",
    ".arw",
    ".cr2",
    ".cr3",
    ".crw",
    ".dcr",
    ".dng",
    ".erf",
    ".fff",
    ".gpr",
    ".iiq",
    ".kdc",
    ".mef",
    ".mos",
    ".mrw",
    ".nef",
    ".nrw",
    ".orf",
    ".pef",
    ".raf",
    ".raw",
    ".rw2",
    ".rwl",
    ".sr2",
    ".srf",
    ".srw",
    ".x3f",
}

VIDEO_EXTENSIONS = {
    ".3gp",
    ".avi",
    ".m2ts",
    ".m4v",
    ".mov",
    ".mp4",
    ".mpeg",
    ".mpg",
    ".mts",
    ".webm",
    ".wmv",
}

DEFAULT_MAX_LONG_EDGE = int(os.environ.get("GENIUSAI_CONVERT_MAX_EDGE", "2048"))
DEFAULT_JPEG_QUALITY = int(os.environ.get("GENIUSAI_CONVERT_QUALITY", "85"))

# LibRaw flip values (raw.sizes.flip) → PIL transpose operations.
_LIBRAW_FLIP_TRANSPOSE = {
    3: Image.Transpose.ROTATE_180,
    5: Image.Transpose.ROTATE_90,  # 90° CCW
    6: Image.Transpose.ROTATE_270,  # 90° CW
}


def _extension(filename: str | None) -> str:
    if not filename:
        return ""
    return os.path.splitext(filename)[1].lower()


def is_raw_filename(filename: str | None) -> bool:
    return _extension(filename) in RAW_EXTENSIONS


def is_video_filename(filename: str | None) -> bool:
    return _extension(filename) in VIDEO_EXTENSIONS


def _jpeg_filename(filename: str | None) -> str:
    base = os.path.splitext(os.path.basename(filename or "photo"))[0]
    return base + ".jpg"


def _downscale(image: Image.Image, max_long_edge: int) -> Image.Image:
    long_edge = max(image.size)
    if long_edge <= max_long_edge:
        return image
    scale = max_long_edge / float(long_edge)
    new_size = (
        max(1, int(image.size[0] * scale)),
        max(1, int(image.size[1] * scale)),
    )
    return image.resize(new_size, Image.Resampling.LANCZOS)


def _encode_jpeg(image: Image.Image, quality: int) -> bytes:
    buffer = io.BytesIO()
    image.convert("RGB").save(buffer, format="JPEG", quality=quality)
    return buffer.getvalue()


def _orient_thumb(image: Image.Image, libraw_flip: int) -> Image.Image:
    """Apply orientation to an embedded raw preview.

    Prefer the thumbnail's own EXIF orientation tag when present; otherwise
    fall back to the LibRaw flip flag so portrait shots don't index sideways.
    """
    exif = image.getexif()
    if exif.get(0x0112):  # EXIF Orientation tag present in the embedded JPEG
        return ImageOps.exif_transpose(image)
    transpose = _LIBRAW_FLIP_TRANSPOSE.get(libraw_flip)
    if transpose is not None:
        return image.transpose(transpose)
    return image


def _convert_raw(data: bytes, filename: str, max_long_edge: int, quality: int) -> bytes:
    if not _RAWPY_AVAILABLE:
        raise UnsupportedImageError(
            f"Cannot decode raw file '{filename}': raw decoding (rawpy) is not "
            "available in this backend build"
        )
    try:
        with rawpy.imread(io.BytesIO(data)) as raw:
            flip = getattr(raw.sizes, "flip", 0)
            image = None
            try:
                thumb = raw.extract_thumb()
                if thumb.format == rawpy.ThumbFormat.JPEG:
                    image = _orient_thumb(Image.open(io.BytesIO(thumb.data)), flip)
                elif thumb.format == rawpy.ThumbFormat.BITMAP:
                    image = Image.fromarray(thumb.data)
            except Exception as exc:
                logger.debug(
                    "No usable embedded preview in %s (%s); falling back to demosaic",
                    filename,
                    exc,
                )
            if image is None:
                # Full decode as last resort; half_size trades resolution for
                # a ~4x speedup and is plenty for embeddings/faces/LLM input.
                rgb = raw.postprocess(half_size=True, use_camera_wb=True)
                image = Image.fromarray(rgb)
            image = _downscale(image, max_long_edge)
            return _encode_jpeg(image, quality)
    except UnsupportedImageError:
        raise
    except Exception as exc:
        raise UnsupportedImageError(
            f"Failed to decode raw file '{filename}': {exc}"
        ) from exc


def normalize_image_bytes(
    data: bytes,
    filename: str | None,
    max_long_edge: int = DEFAULT_MAX_LONG_EDGE,
    quality: int = DEFAULT_JPEG_QUALITY,
) -> tuple[bytes, str]:
    """
    Return (jpeg_ready_bytes, filename) for the indexing pipeline.

    JPEG/PNG bytes within the size limit are returned unchanged. Raw files are
    converted via their embedded preview (or demosaiced). Anything else Pillow
    can open (HEIC/TIFF/oversized JPEG) is downscaled and re-encoded.

    Raises UnsupportedImageError when the bytes cannot be turned into an image.
    """
    filename = filename or "photo"

    if not data:
        raise UnsupportedImageError(f"Empty image data for '{filename}'")

    if is_video_filename(filename):
        raise UnsupportedImageError(
            f"'{filename}' is a video file and cannot be indexed"
        )

    if is_raw_filename(filename):
        # Never let Pillow "open" TIFF-based raws (NEF/CR2/DNG) — it returns
        # tiny thumbnails or unprocessed sensor data.
        return _convert_raw(data, filename, max_long_edge, quality), _jpeg_filename(
            filename
        )

    try:
        with Image.open(io.BytesIO(data)) as probe:
            image_format = probe.format
            size = probe.size
    except Exception:
        # Extension didn't say raw, but the bytes might still be (e.g. wrong
        # extension). Give rawpy one chance before failing.
        if _RAWPY_AVAILABLE:
            try:
                return _convert_raw(
                    data, filename, max_long_edge, quality
                ), _jpeg_filename(filename)
            except UnsupportedImageError:
                pass
        raise UnsupportedImageError(
            f"Unsupported or corrupt image data for '{filename}'"
        )

    if image_format in ("JPEG", "PNG") and max(size) <= max_long_edge:
        return data, filename

    try:
        with Image.open(io.BytesIO(data)) as image:
            oriented = ImageOps.exif_transpose(image)
            oriented = _downscale(oriented, max_long_edge)
            return _encode_jpeg(oriented, quality), _jpeg_filename(filename)
    except Exception as exc:
        raise UnsupportedImageError(
            f"Failed to convert image '{filename}': {exc}"
        ) from exc
