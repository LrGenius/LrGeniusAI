import base64
import io
import unittest

from PIL import Image

from providers.base import LLMProviderBase


class _StubProvider(LLMProviderBase):
    def generate_metadata(self, request):
        raise NotImplementedError

    def is_available(self):
        return True

    def generate_edit_recipe(self, request):
        raise NotImplementedError

    def list_available_models(self):
        return []


def _make_jpeg_bytes(color=(255, 0, 0)):
    buf = io.BytesIO()
    Image.new("RGB", (8, 8), color).save(buf, format="JPEG", quality=80)
    return buf.getvalue()


def _make_png_bytes(color=(0, 255, 0)):
    buf = io.BytesIO()
    Image.new("RGB", (8, 8), color).save(buf, format="PNG")
    return buf.getvalue()


class ImageToBase64Tests(unittest.TestCase):
    def setUp(self):
        self.provider = _StubProvider({})

    def test_jpeg_input_skips_re_encode(self):
        jpeg_bytes = _make_jpeg_bytes()
        result = self.provider._image_to_base64(jpeg_bytes)
        # Fast path: returned base64 must decode back to the exact original bytes
        self.assertEqual(base64.b64decode(result), jpeg_bytes)

    def test_jpeg_magic_number_detected(self):
        jpeg_bytes = _make_jpeg_bytes()
        self.assertTrue(jpeg_bytes.startswith(b"\xff\xd8\xff"))

    def test_png_re_encoded_to_jpeg(self):
        png_bytes = _make_png_bytes()
        result = self.provider._image_to_base64(png_bytes)
        decoded = base64.b64decode(result)
        # Should now be a JPEG, not the original PNG bytes
        self.assertNotEqual(decoded, png_bytes)
        self.assertTrue(decoded.startswith(b"\xff\xd8\xff"))
        # And should still decode back to a valid image
        Image.open(io.BytesIO(decoded)).verify()

    def test_returns_valid_base64(self):
        result = self.provider._image_to_base64(_make_jpeg_bytes())
        self.assertIsInstance(result, str)
        # No exception means valid base64
        base64.b64decode(result, validate=True)

    def test_non_image_bytes_raises_value_error(self):
        with self.assertRaises(ValueError):
            self.provider._image_to_base64(b"not an image at all")

    def test_empty_bytes_raises_value_error(self):
        with self.assertRaises(ValueError):
            self.provider._image_to_base64(b"")

    def test_jpeg_fast_path_trusts_magic_header(self):
        # Pin current behavior: the fast path doesn't validate the JPEG body.
        # A header followed by garbage is still encoded without error.
        bogus = b"\xff\xd8\xff" + b"garbage payload"
        result = self.provider._image_to_base64(bogus)
        self.assertEqual(base64.b64decode(result), bogus)


if __name__ == "__main__":
    unittest.main()
