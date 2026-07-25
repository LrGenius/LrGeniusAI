"""Tests for utils.image_convert.normalize_image_bytes — the ingestion
normalization used by /index, /index_base64 and /index_by_reference."""

import io

import pytest
from PIL import Image

from utils import image_convert
from utils.image_convert import UnsupportedImageError, normalize_image_bytes


def _jpeg_bytes(width=64, height=48, color=(120, 30, 200)):
    buf = io.BytesIO()
    Image.new("RGB", (width, height), color).save(buf, format="JPEG")
    return buf.getvalue()


def _png_bytes(width=64, height=48):
    buf = io.BytesIO()
    Image.new("RGB", (width, height)).save(buf, format="PNG")
    return buf.getvalue()


def test_small_jpeg_passes_through_unchanged():
    data = _jpeg_bytes()
    out, name = normalize_image_bytes(data, "photo.jpg", max_long_edge=2048)
    assert out == data
    assert name == "photo.jpg"


def test_small_png_passes_through_unchanged():
    data = _png_bytes()
    out, name = normalize_image_bytes(data, "photo.png", max_long_edge=2048)
    assert out == data
    assert name == "photo.png"


def test_oversized_jpeg_is_downscaled_and_reencoded():
    data = _jpeg_bytes(width=400, height=200)
    out, name = normalize_image_bytes(data, "big.jpg", max_long_edge=100)
    assert out != data
    assert name == "big.jpg"
    with Image.open(io.BytesIO(out)) as img:
        assert max(img.size) == 100
        assert img.format == "JPEG"


def test_tiff_is_converted_to_jpeg():
    buf = io.BytesIO()
    Image.new("RGB", (32, 32)).save(buf, format="TIFF")
    out, name = normalize_image_bytes(buf.getvalue(), "scan.tif", max_long_edge=2048)
    assert name == "scan.jpg"
    with Image.open(io.BytesIO(out)) as img:
        assert img.format == "JPEG"


def test_empty_data_raises():
    with pytest.raises(UnsupportedImageError):
        normalize_image_bytes(b"", "photo.jpg")


def test_video_extension_raises():
    with pytest.raises(UnsupportedImageError, match="video"):
        normalize_image_bytes(b"whatever", "clip.mp4")


def test_garbage_bytes_raise():
    with pytest.raises(UnsupportedImageError):
        normalize_image_bytes(b"not an image at all", "photo.jpg")


def test_garbage_raw_bytes_raise_clean_error():
    with pytest.raises(UnsupportedImageError, match="IMG_0001.NEF"):
        normalize_image_bytes(b"definitely not a nef", "IMG_0001.NEF")


def test_raw_extension_uses_raw_converter(monkeypatch):
    called = {}

    def fake_convert(data, filename, max_long_edge, quality):
        called["filename"] = filename
        return _jpeg_bytes()

    monkeypatch.setattr(image_convert, "_convert_raw", fake_convert)
    out, name = normalize_image_bytes(b"raw sensor data", "IMG_0001.CR3")
    assert called["filename"] == "IMG_0001.CR3"
    assert name == "IMG_0001.jpg"
    with Image.open(io.BytesIO(out)) as img:
        assert img.format == "JPEG"


def test_raw_without_rawpy_reports_helpful_error(monkeypatch):
    monkeypatch.setattr(image_convert, "_RAWPY_AVAILABLE", False)
    with pytest.raises(UnsupportedImageError, match="rawpy"):
        normalize_image_bytes(b"raw sensor data", "IMG_0001.ARW")


def test_is_raw_filename_case_insensitive():
    assert image_convert.is_raw_filename("a.NEF")
    assert image_convert.is_raw_filename("b.cr3")
    assert not image_convert.is_raw_filename("c.jpg")
    assert not image_convert.is_raw_filename(None)
