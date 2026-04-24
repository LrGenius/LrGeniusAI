"""
EXIF/IPTC location tag extraction service.

Lightroom writes reverse-geocoded location data (City, State, Country) from its
internal geocoding cache into JPEG IPTC/XMP metadata on export.  This module
extracts those tags so the backend can pass precise, human-readable location
context to the LLM instead of raw GPS coordinates.

Supported tag sources (in priority order):
  1. IPTC Application2 (IIM) tags embedded by Lightroom
  2. XMP Photoshop namespace (photoshop:City / State / Country)
  3. GPS IFD (latitude / longitude) as numeric fallback
"""

from __future__ import annotations

import struct
import io
from typing import Optional

from config import logger


# ---------------------------------------------------------------------------
# IPTC IIM record-2 tag IDs relevant for location
# ---------------------------------------------------------------------------
_IPTC_LOCATION_ID = 0x5C  # 92  – Sub-location
_IPTC_CITY_ID = 0x5A  # 90  – City
_IPTC_STATE_ID = 0x5F  # 95  – Province-State
_IPTC_COUNTRY_CODE_ID = 0x64  # 100 – Country Code
_IPTC_COUNTRY_ID = 0x65  # 101 – Country Name

_IPTC_LOCATION_TAGS = {
    _IPTC_LOCATION_ID,
    _IPTC_CITY_ID,
    _IPTC_STATE_ID,
    _IPTC_COUNTRY_CODE_ID,
    _IPTC_COUNTRY_ID,
}


def _parse_iptc(data: bytes) -> dict[int, str]:
    """Parse raw IPTC IIM (record-2) bytes into a tag_id → value mapping."""
    result: dict[int, str] = {}
    i = 0
    while i + 4 < len(data):
        if data[i] != 0x1C:
            i += 1
            continue
        record = data[i + 1]
        tag = data[i + 2]
        size = struct.unpack(">H", data[i + 3 : i + 5])[0]
        i += 5
        if record == 2 and tag in _IPTC_LOCATION_TAGS:
            try:
                value = data[i : i + size].decode("utf-8", errors="replace").strip()
                if value:
                    result[tag] = value
            except Exception:
                pass
        i += size
    return result


def _read_iptc_from_jpeg(image_bytes: bytes) -> dict[int, str]:
    """
    Scan JPEG markers for APP13 (Photoshop/IPTC) and parse IPTC record 2.
    Returns a mapping of IPTC tag_id → decoded string value.
    """
    buf = io.BytesIO(image_bytes)
    try:
        marker = buf.read(2)
        if marker != b"\xff\xd8":
            return {}

        while True:
            hdr = buf.read(2)
            if len(hdr) < 2:
                break
            if hdr[0] != 0xFF:
                break
            marker_byte = hdr[1]
            if marker_byte in (0xD8, 0xD9, 0xDA):
                break  # SOI, EOI, SOS – stop scanning
            length_bytes = buf.read(2)
            if len(length_bytes) < 2:
                break
            length = struct.unpack(">H", length_bytes)[0] - 2
            segment_data = buf.read(length)
            if marker_byte == 0xED:  # APP13
                # Photoshop 3.0 IRB begins with "Photoshop 3.0\x00"
                header = b"Photoshop 3.0\x00"
                if segment_data.startswith(header):
                    irb = segment_data[len(header) :]
                    j = 0
                    while j + 12 <= len(irb):
                        if irb[j : j + 4] != b"8BIM":
                            j += 1
                            continue
                        resource_id = struct.unpack(">H", irb[j + 4 : j + 6])[0]
                        # Pascal string (name) – skip
                        name_len = irb[j + 6]
                        name_skip = name_len + (1 if name_len % 2 == 0 else 0)
                        data_offset = j + 7 + name_skip
                        if data_offset + 4 > len(irb):
                            break
                        data_len = struct.unpack(
                            ">I", irb[data_offset : data_offset + 4]
                        )[0]
                        data_start = data_offset + 4
                        resource_data = irb[data_start : data_start + data_len]
                        if resource_id == 0x0404:  # IPTC-NAA resource
                            return _parse_iptc(resource_data)
                        j = data_start + data_len
                        if data_len % 2:
                            j += 1
    except Exception as exc:
        logger.debug("EXIF/IPTC parse error: %s", exc)
    return {}


def _dms_to_decimal(dms_tuple) -> Optional[float]:
    """
    Convert an EXIF DMS tuple (degrees, minutes, seconds) to decimal degrees.
    Each element may be an IFDRational, a tuple (num, den), or a plain number.
    """
    try:

        def _to_float(v) -> float:
            if isinstance(v, tuple):
                return v[0] / v[1] if v[1] else 0.0
            # IFDRational from pillow
            return float(v)

        deg, mins, secs = dms_tuple
        return _to_float(deg) + _to_float(mins) / 60.0 + _to_float(secs) / 3600.0
    except Exception:
        return None


def _read_gps_from_exif(image_bytes: bytes) -> tuple[Optional[float], Optional[float]]:
    """
    Read GPS latitude and longitude from JPEG EXIF using Pillow.
    Returns (latitude, longitude) as floats, or (None, None) on failure.
    """
    try:
        from PIL import Image
        from PIL.ExifTags import TAGS, GPSTAGS

        img = Image.open(io.BytesIO(image_bytes))
        exif_data = img._getexif()  # type: ignore[attr-defined]
        if not exif_data:
            return None, None

        # Find GPS IFD
        gps_ifd = None
        for tag_id, value in exif_data.items():
            tag_name = TAGS.get(tag_id, "")
            if tag_name == "GPSInfo":
                gps_ifd = value
                break

        if not gps_ifd:
            return None, None

        gps = {}
        for tag_id, value in gps_ifd.items():
            gps[GPSTAGS.get(tag_id, tag_id)] = value

        lat_dms = gps.get("GPSLatitude")
        lat_ref = gps.get("GPSLatitudeRef", "N")
        lon_dms = gps.get("GPSLongitude")
        lon_ref = gps.get("GPSLongitudeRef", "E")

        if lat_dms is None or lon_dms is None:
            return None, None

        lat = _dms_to_decimal(lat_dms)
        lon = _dms_to_decimal(lon_dms)
        if lat is None or lon is None:
            return None, None

        if str(lat_ref).upper() == "S":
            lat = -lat
        if str(lon_ref).upper() == "W":
            lon = -lon

        return lat, lon
    except Exception as exc:
        logger.debug("GPS EXIF read error: %s", exc)
        return None, None


def extract_location_tags(image_bytes: bytes) -> Optional[dict]:
    """
    Extract all relevant location tags from a JPEG's IPTC/EXIF metadata.

    Returns a dict with any of the following keys that were found:
        location  – sub-location / landmark (e.g. "Chiemsee")
        city      – city name  (e.g. "Prien am Chiemsee")
        state     – province / state  (e.g. "Bayern")
        country   – country name  (e.g. "Deutschland")
        country_code – ISO 3166-1 alpha-2 code  (e.g. "DE")
        gps_latitude  – decimal degrees float
        gps_longitude – decimal degrees float

    Returns None if no location data was found at all.
    """
    result: dict = {}

    # --- IPTC tags (reverse-geocoded data written by Lightroom) ---
    iptc = _read_iptc_from_jpeg(image_bytes)
    if iptc.get(_IPTC_LOCATION_ID):
        result["location"] = iptc[_IPTC_LOCATION_ID]
    if iptc.get(_IPTC_CITY_ID):
        result["city"] = iptc[_IPTC_CITY_ID]
    if iptc.get(_IPTC_STATE_ID):
        result["state"] = iptc[_IPTC_STATE_ID]
    if iptc.get(_IPTC_COUNTRY_ID):
        result["country"] = iptc[_IPTC_COUNTRY_ID]
    if iptc.get(_IPTC_COUNTRY_CODE_ID):
        result["country_code"] = iptc[_IPTC_COUNTRY_CODE_ID]

    # --- GPS coordinates (numeric fallback / complement) ---
    lat, lon = _read_gps_from_exif(image_bytes)
    if lat is not None:
        result["gps_latitude"] = round(lat, 6)
    if lon is not None:
        result["gps_longitude"] = round(lon, 6)

    if not result:
        return None

    logger.debug(
        "Extracted location tags: %s",
        ", ".join(f"{k}={v}" for k, v in result.items()),
    )
    return result


def format_location_for_prompt(location_data: dict) -> Optional[str]:
    """
    Format extracted location data into a human-readable string for the LLM prompt.

    Priority: structured place names > GPS coordinates.

    Examples:
        "Chiemsee, Prien am Chiemsee, Bayern, Deutschland"
        "Bayern, Deutschland"
        "47.855600, 12.365700"
    """
    if not location_data:
        return None

    parts = []
    if location_data.get("location"):
        parts.append(location_data["location"])
    if location_data.get("city"):
        parts.append(location_data["city"])
    if location_data.get("state"):
        parts.append(location_data["state"])
    if location_data.get("country"):
        parts.append(location_data["country"])

    if parts:
        return ", ".join(parts)

    # Fallback: GPS only
    lat = location_data.get("gps_latitude")
    lon = location_data.get("gps_longitude")
    if lat is not None and lon is not None:
        return f"{lat:.6f}, {lon:.6f}"

    return None
