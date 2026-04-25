import unittest

from service_index import _safe_unit_interval


class SafeUnitIntervalTests(unittest.TestCase):
    def test_in_range_returned_unchanged(self):
        self.assertEqual(_safe_unit_interval(0.0), 0.0)
        self.assertEqual(_safe_unit_interval(0.5), 0.5)
        self.assertEqual(_safe_unit_interval(1.0), 1.0)

    def test_below_zero_clamped_up(self):
        self.assertEqual(_safe_unit_interval(-0.1), 0.0)
        self.assertEqual(_safe_unit_interval(-100.0), 0.0)

    def test_above_one_clamped_down(self):
        self.assertEqual(_safe_unit_interval(1.1), 1.0)
        self.assertEqual(_safe_unit_interval(99.9), 1.0)

    def test_int_coerced_to_float(self):
        result = _safe_unit_interval(0)
        self.assertEqual(result, 0.0)
        self.assertIsInstance(result, float)

    def test_string_numeric_coerced(self):
        # float() accepts numeric strings; pin this behavior
        self.assertEqual(_safe_unit_interval("0.5"), 0.5)

    def test_nan_clamps_to_one(self):
        # NaN comparisons in min/max are order-dependent in Python: with the
        # current implementation `max(0.0, min(1.0, nan))` returns 1.0 because
        # `nan < 1.0` is False, so min keeps 1.0, and max(0.0, 1.0) is 1.0.
        # Pin this behavior so a refactor to short-circuit NaN is intentional.
        self.assertEqual(_safe_unit_interval(float("nan")), 1.0)

    def test_non_numeric_raises(self):
        with self.assertRaises((TypeError, ValueError)):
            _safe_unit_interval("not a number")
        with self.assertRaises(TypeError):
            _safe_unit_interval(None)


if __name__ == "__main__":
    unittest.main()
