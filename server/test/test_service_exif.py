"""Tests for services/exif.py — IPTC parsing, DMS conversion, location formatting,
and the extract_location_tags entry point."""

import struct

import pytest

from services.exif import (
    _dms_to_decimal,
    _parse_iptc,
    extract_location_tags,
    format_location_for_prompt,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# IPTC record-2 marker bytes
_IPTC_RECORD = 0x02
_TAG_CITY = 0x5A  # 90
_TAG_COUNTRY = 0x65  # 101
_TAG_LOCATION = 0x5C  # 92
_TAG_STATE = 0x5F  # 95
_TAG_COUNTRY_CODE = 0x64  # 100


def _make_iptc_record(tag_id: int, value_bytes: bytes) -> bytes:
    """Build a single IPTC IIM record-2 marker: 0x1C 0x02 <tag> <2-byte len> <data>."""
    return (
        bytes([0x1C, _IPTC_RECORD, tag_id])
        + struct.pack(">H", len(value_bytes))
        + value_bytes
    )


# ---------------------------------------------------------------------------
# _parse_iptc
# ---------------------------------------------------------------------------


class TestParseIptc:
    def test_empty_bytes_returns_empty_dict(self):
        assert _parse_iptc(b"") == {}

    def test_parses_city_tag(self):
        city = b"Berlin"
        data = _make_iptc_record(_TAG_CITY, city)
        result = _parse_iptc(data)
        assert result[_TAG_CITY] == "Berlin"

    def test_parses_country_tag(self):
        data = _make_iptc_record(_TAG_COUNTRY, b"Deutschland")
        result = _parse_iptc(data)
        assert result[_TAG_COUNTRY] == "Deutschland"

    def test_parses_multiple_tags(self):
        data = _make_iptc_record(_TAG_CITY, b"Munich") + _make_iptc_record(
            _TAG_COUNTRY, b"Germany"
        )
        result = _parse_iptc(data)
        assert result[_TAG_CITY] == "Munich"
        assert result[_TAG_COUNTRY] == "Germany"

    def test_non_record_2_tag_is_skipped(self):
        # Use record byte 0x03 instead of 0x02 — should be skipped.
        bad_record = bytes([0x1C, 0x03, _TAG_CITY]) + struct.pack(">H", 4) + b"Nope"
        result = _parse_iptc(bad_record)
        assert result == {}

    def test_tag_not_in_location_tags_is_skipped(self):
        # Tag 0x01 (object name) is not a location tag.
        unknown_tag = bytes([0x1C, 0x02, 0x01]) + struct.pack(">H", 5) + b"Title"
        result = _parse_iptc(unknown_tag)
        assert result == {}

    def test_location_tag_parsed(self):
        data = _make_iptc_record(_TAG_LOCATION, b"Chiemsee")
        result = _parse_iptc(data)
        assert result[_TAG_LOCATION] == "Chiemsee"

    def test_state_tag_parsed(self):
        data = _make_iptc_record(_TAG_STATE, b"Bayern")
        result = _parse_iptc(data)
        assert result[_TAG_STATE] == "Bayern"

    def test_country_code_tag_parsed(self):
        data = _make_iptc_record(_TAG_COUNTRY_CODE, b"DE")
        result = _parse_iptc(data)
        assert result[_TAG_COUNTRY_CODE] == "DE"

    def test_whitespace_stripped_from_value(self):
        data = _make_iptc_record(_TAG_CITY, b"  Paris  ")
        result = _parse_iptc(data)
        assert result[_TAG_CITY] == "Paris"

    def test_empty_value_not_included(self):
        data = _make_iptc_record(_TAG_CITY, b"   ")
        result = _parse_iptc(data)
        assert _TAG_CITY not in result

    def test_garbage_bytes_between_records_skipped(self):
        """Parser should skip bytes that don't start with 0x1C."""
        city = _make_iptc_record(_TAG_CITY, b"Rome")
        data = b"\x00\xff\xab" + city
        result = _parse_iptc(data)
        assert result[_TAG_CITY] == "Rome"


# ---------------------------------------------------------------------------
# _dms_to_decimal
# ---------------------------------------------------------------------------


class TestDmsToDecimal:
    def test_whole_degrees(self):
        result = _dms_to_decimal((48, 0, 0))
        assert result == pytest.approx(48.0)

    def test_degrees_and_minutes(self):
        result = _dms_to_decimal((48, 8, 0))
        assert result == pytest.approx(48.0 + 8 / 60.0)

    def test_degrees_minutes_seconds(self):
        result = _dms_to_decimal((48, 8, 30))
        assert result == pytest.approx(48.0 + 8 / 60.0 + 30 / 3600.0)

    def test_tuple_of_tuples_rational_form(self):
        # Pillow IFDRational stored as (numerator, denominator) tuples.
        dms = ((48, 1), (8, 1), (0, 1))
        result = _dms_to_decimal(dms)
        assert result == pytest.approx(48.0 + 8 / 60.0)

    def test_fractional_seconds(self):
        dms = ((12, 1), (30, 1), (18, 2))  # 12 deg, 30 min, 9 sec
        result = _dms_to_decimal(dms)
        assert result == pytest.approx(12.0 + 30 / 60.0 + 9 / 3600.0)

    def test_bad_input_returns_none(self):
        assert _dms_to_decimal(None) is None

    def test_too_few_elements_returns_none(self):
        assert _dms_to_decimal((10, 20)) is None

    def test_non_numeric_returns_none(self):
        assert _dms_to_decimal(("a", "b", "c")) is None

    def test_zero_denominator_in_tuple_treated_as_zero(self):
        # (num, 0) denominator should be treated as 0.0, not raise.
        dms = ((0, 0), (0, 1), (0, 1))
        result = _dms_to_decimal(dms)
        assert result == pytest.approx(0.0)


# ---------------------------------------------------------------------------
# format_location_for_prompt
# ---------------------------------------------------------------------------


class TestFormatLocationForPrompt:
    def test_none_returns_none(self):
        assert format_location_for_prompt(None) is None

    def test_empty_dict_returns_none(self):
        assert format_location_for_prompt({}) is None

    def test_city_and_country(self):
        result = format_location_for_prompt({"city": "Berlin", "country": "Germany"})
        assert result == "Berlin, Germany"

    def test_full_structured_fields(self):
        data = {
            "location": "Chiemsee",
            "city": "Prien am Chiemsee",
            "state": "Bayern",
            "country": "Deutschland",
        }
        result = format_location_for_prompt(data)
        assert result == "Chiemsee, Prien am Chiemsee, Bayern, Deutschland"

    def test_location_only(self):
        result = format_location_for_prompt({"location": "Eiffel Tower"})
        assert result == "Eiffel Tower"

    def test_state_and_country(self):
        result = format_location_for_prompt({"state": "Bayern", "country": "Germany"})
        assert result == "Bayern, Germany"

    def test_gps_only_fallback(self):
        result = format_location_for_prompt(
            {"gps_latitude": 47.8556, "gps_longitude": 12.3657}
        )
        assert result == "47.855600, 12.365700"

    def test_gps_negative_coordinates(self):
        result = format_location_for_prompt(
            {"gps_latitude": -33.8688, "gps_longitude": 151.2093}
        )
        assert result == "-33.868800, 151.209300"

    def test_structured_takes_priority_over_gps(self):
        data = {
            "city": "Sydney",
            "gps_latitude": -33.8688,
            "gps_longitude": 151.2093,
        }
        result = format_location_for_prompt(data)
        assert result == "Sydney"

    def test_partial_gps_missing_longitude_returns_none(self):
        result = format_location_for_prompt({"gps_latitude": 48.0})
        assert result is None

    def test_partial_gps_missing_latitude_returns_none(self):
        result = format_location_for_prompt({"gps_longitude": 11.0})
        assert result is None

    def test_country_code_not_included_in_output(self):
        # country_code is metadata but not displayed in the prompt string.
        result = format_location_for_prompt({"city": "Berlin", "country_code": "DE"})
        # country_code alone → no structured parts → GPS fallback (none here) → None
        assert result == "Berlin"


# ---------------------------------------------------------------------------
# extract_location_tags
# ---------------------------------------------------------------------------


class TestExtractLocationTags:
    def test_empty_bytes_returns_none(self):
        result = extract_location_tags(b"")
        assert result is None

    def test_random_bytes_returns_none(self):
        result = extract_location_tags(b"\x00\x01\x02\x03\x04\x05")
        assert result is None

    def test_non_jpeg_returns_none(self):
        # PNG magic bytes — no JPEG marker, no EXIF.
        png_magic = b"\x89PNG\r\n\x1a\n" + b"\x00" * 50
        result = extract_location_tags(png_magic)
        assert result is None

    def test_minimal_jpeg_without_location_returns_none(self):
        # Minimal valid JPEG: SOI + EOI markers only.
        minimal_jpeg = b"\xff\xd8\xff\xd9"
        result = extract_location_tags(minimal_jpeg)
        assert result is None
