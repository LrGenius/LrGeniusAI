"""Extract a small, scrubbed set of ``getDevelopSettings()`` fixtures from a training dump.

Input is the JSON written by ``dump_training.py``: a list of rows whose
``metadata`` field is a JSON string holding, among photo metadata, the
``develop_settings`` blob - itself a JSON string produced by the plugin's
``JSON.lua`` from ``photo:getDevelopSettings()``.

The tool picks a handful of rows that together cover the shapes the Lua/JSON
reader of ``lrg-develop`` must handle, scrubs them, and writes one file per
shape plus ``manifest.json`` (what each file covers) to ``--out-dir``; the
repository keeps them in ``server-rs/testdata/develop/lua/``. For each shape it
takes the smallest row that has it, avoiding rows already taken; shapes that
need no file of their own are satisfied by re-picking an existing file or, only
if that fails, by one more file.

**Only this tool's output may be committed.** The dump itself is private.

What is written
---------------
Only the decoded ``develop_settings`` value. Every other metadata field (photo
id, file name, capture time, camera, EXIF, scene tags, canonical settings) is
dropped. Numbers are written with the exact token ``JSON.lua`` produced, so int
vs float, ``true``/``false`` vs ``0``/``1`` and ``[]`` for empty tables survive
unchanged.

How it is scrubbed (the repository is public)
---------------------------------------------
* ``Look.Parameters`` (Adobe's or a third party's profile definition) becomes
  a stub holding only the original ``ConvertToGrayscale`` and ``ProcessVersion``
  (when present). ``Look.Name``/``UUID`` are kept only for the Adobe profiles in
  ``ADOBE_LOOKS`` - UUIDs verified to be ``crs:UUID`` of a profile in Lightroom
  Classic's bundled ``Adobe/Profiles``. Any other profile's name and UUID are
  replaced, whatever its copyright text says; ``--stub-look-uuid`` replaces the
  Adobe ones too. Copyright, group, sort name, cluster and camera restriction
  always become generic text.
* Keys ``Table_<md5>``, ``BrushTable_<md5>`` (and the ``MaskBrushTable``
  reference to them), ``LookTable*`` and ``RGBTable*`` are removed everywhere.
* Every 32-hex digest/ID (``MaskDigest``, ``InputDigest``, ``CorrectionSyncID``,
  ``LensProfileDigest``, ``AILookData``, ``pm_patch``, ...) becomes ``0`` x 20 +
  12 hex digits of a counter, every GUID (``CorrectionID``, ``MaskID``) becomes
  ``00000000-0000-4000-8000-`` + 12 hex digits. The mapping is consistent across
  all files, so equal IDs stay equal but nothing links back to a catalog.
* ``GrainSeed`` (a random per-photo value) becomes the constant 4000000001.
* Free text becomes type-neutral generic English: ``CorrectionName`` is
  "Correction N", ``MaskName`` "Mask N" (LrC's own defaults are type-neutral too,
  so a reader cannot cheat by classifying masks by name); lens profile names
  and file names, model versions and time stamps get fixed synthetic values.
* Every other string must satisfy the value rule of its key (``STRING_RULES``)
  or be a number list or brush-dab string; anything else becomes
  "Synthetic Text" and is reported. The scrubber and the hygiene check share
  these rules.
* Numeric values are otherwise unmodified; they can reveal the camera class
  (image size, black levels) and focal length.

Safety
------
The run writes into a private temporary folder first, checks it there and only
then replaces the files it owns in ``--out-dir`` (those listed in the previous
``manifest.json``). If the hygiene check or the shape check fails, ``--out-dir``
is left untouched and the rejected output is deleted (``--keep-rejected`` keeps
it for inspection, in a folder only you can read). Hand-written fixtures in
``--out-dir`` must be named ``hand_*.json``; the tool never deletes them and
checks them with the relaxed rules below. Inside the repository ``--out-dir``
must be below ``server-rs/testdata/develop/``.

Hygiene check
-------------
Hard failures in every checked file (text level): ``md5p:``, ``file:``, user,
volume, home, ``/private/``, ``/tmp/``, ``/var/``, ``/Library/``, drive and UNC
paths, raw/image file names, ``Table_``, ``LookTable``, ``RGBTable``.
Structural rules for every JSON file: keys are identifiers (language tags only
under ``Group``/``SortName``); ``Look.Parameters`` is the stub; ``Look.UUID``/
``Name`` are an ``ADOBE_LOOKS`` pair or synthetic; every 32-hex value and GUID is
synthetic; no dates; no number or numeric string >= 1e9 except version keys
and the synthetic ``GrainSeed``. Generated files additionally require every
string to satisfy its key's rule. ``--check-only`` runs just the check; with
``--root`` it also walks every ``*.json``/``*.xmp`` below that folder.

Example (``$OUT`` is a folder outside the repository)::

    uv run extract_fixtures.py "$OUT/training_rows.json" --out-dir ../../testdata/develop/lua
    python3 extract_fixtures.py --check-only --out-dir ../../testdata/develop/lua --root ../../testdata/develop
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import tempfile
from collections import Counter
from collections.abc import Callable
from pathlib import Path

from outpath import output_problem

# ---------------------------------------------------------------------------
# Exact-token JSON: numbers keep the text JSON.lua wrote
# ---------------------------------------------------------------------------


class RawInt(int):
    """An int that remembers its JSON token."""

    raw: str

    def __new__(cls, text: str) -> RawInt:
        obj = super().__new__(cls, int(text))
        obj.raw = text
        return obj


class RawFloat(float):
    """A float that remembers its JSON token (``1e-05`` stays ``1e-05``)."""

    raw: str

    def __new__(cls, text: str) -> RawFloat:
        obj = super().__new__(cls, float(text))
        obj.raw = text
        return obj


def loads_exact(text: str):
    return json.loads(text, parse_int=RawInt, parse_float=RawFloat)


def dumps_exact(value, indent: int = 2, level: int = 0) -> str:
    """Deterministic JSON writer that emits numbers with their original token."""
    pad = " " * (indent * (level + 1))
    end = " " * (indent * level)
    if value is True:
        return "true"
    if value is False:
        return "false"
    if value is None:
        return "null"
    if isinstance(value, (RawInt, RawFloat)):
        return value.raw
    if isinstance(value, (int, float)):
        return json.dumps(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        if not value:
            return "[]"
        return "[\n" + ",\n".join(pad + dumps_exact(v, indent, level + 1) for v in value) + "\n" + end + "]"
    if isinstance(value, dict):
        if not value:
            return "{}"
        items = (
            pad + json.dumps(k, ensure_ascii=False) + ": " + dumps_exact(v, indent, level + 1) for k, v in value.items()
        )
        return "{\n" + ",\n".join(items) + "\n" + end + "}"
    raise TypeError(f"cannot encode {type(value).__name__}")


# ---------------------------------------------------------------------------
# Adobe profiles whose name and UUID may be published
# ---------------------------------------------------------------------------

# UUID -> Name of profiles verified to exist as crs:UUID in Lightroom Classic's
# bundled Settings/Adobe/Profiles (LrC 15.5.1). Extend deliberately, only after
# checking the bundle; never derive Adobe-ness from a Look's own text.
ADOBE_LOOKS = {
    "B952C231111CD8E0ECCF14B86BAA7077": "Adobe Color",
    "6F9C877E84273F4E8271E6B91BEB36A1": "Adobe Landscape",
    "0CFE8F8AB5F63B2A73CE0B0077D20817": "Adobe Monochrome",
    "D6496412E06A83789C499DF9540AA616": "Adobe Portrait",
    "EA1DE074F188405965EF399C72C221D9": "Adobe Vivid",
    "226E6263ED94405B973B41F5A6E67BE9": "Adaptive Color",
    "4B2C86502E5F2637A81F96F8D96AC1F9": "Artistic 04",
    "ADB81986EAF84E0999C0112BDE1F93D1": "B&W 03",
    "9D1ED025F3DE4C94BC78481AA67B3C89": "B&W 11",
    "8FD143CED5B04137ACE4B5A63962293F": "B&W 12",
    "7B79093CEC7726D67B352CAD78D1601F": "Modern 07",
    "DA1C3775662D6B6A75F8BC2CEEB3724A": "Modern 08",
    "97291D549FC232787BDFD3353151BB62": "Vintage 01",
}


def is_adobe_look(look: dict) -> bool:
    uuid = look.get("UUID")
    return isinstance(uuid, str) and ADOBE_LOOKS.get(uuid.upper()) == look.get("Name")


# ---------------------------------------------------------------------------
# Shapes
# ---------------------------------------------------------------------------

PEOPLE_PARTS = {2, 3, 4, 5, 6, 7, 8, 9, 11, 12}
LANDSCAPE_CLASSES = {50001, 50002, 50003, 50004, 50005, 50006, 50007, 50008}
BACKGROUND_CATEGORY = 22
LEGACY_LOCAL_KEYS = ("LocalExposure", "LocalContrast", "LocalClarity")


def gesture_kind(mask: dict) -> str | None:
    for g in mask.get("Gesture") or []:
        if isinstance(g, dict):
            return g.get("What")
    return None


def mask_shape(mask: dict) -> str | None:
    """Fixture shape name for one ``CorrectionMasks`` item."""
    what = mask.get("What")
    if what == "Mask/Image":
        sub = mask.get("MaskSubType")
        cat = mask.get("MaskSubCategoryID")
        if sub == 1:
            return "mask_ai_subject"
        if sub == 2:
            return "mask_ai_sky"
        if sub == 3 and cat in PEOPLE_PARTS:
            return "mask_ai_people_part_all"
        if sub == 0:
            if gesture_kind(mask) == "Mask/Polygon":
                return "mask_select_object_polygon"
            if gesture_kind(mask) == "Mask/Paint":
                return "mask_select_object_brush"
            if cat == BACKGROUND_CATEGORY:
                return "mask_ai_background"
            if cat in PEOPLE_PARTS:
                return "mask_ai_people_part_person"
            if cat in LANDSCAPE_CLASSES:
                return "mask_ai_landscape"
        return "mask_ai_other"
    if what == "Mask/Gradient":
        return "mask_linear_gradient"
    if what == "Mask/CircularGradient":
        return "mask_radial_gradient"
    if what == "Mask/Aggregate":
        return "mask_brush_aggregate"
    if what == "Mask/RangeMask":
        crm = mask.get("CorrectionRangeMask") or {}
        return {1: "mask_range_color", 2: "mask_range_luminance"}.get(crm.get("Type"), "mask_range_other")
    return None


def _has_exponent_token(v) -> bool:
    if isinstance(v, RawFloat):
        return "e" in v.raw or "E" in v.raw
    if isinstance(v, dict):
        return any(_has_exponent_token(x) for x in v.values())
    if isinstance(v, list):
        return any(_has_exponent_token(x) for x in v)
    return False


def shapes_of(d) -> set[str]:
    """Every fixture shape a decoded ``develop_settings`` value exhibits."""
    if isinstance(d, list):
        return {"top_level_empty"} if not d else set()
    s: set[str] = set()
    pv = d.get("ProcessVersion")
    if isinstance(pv, str):
        s.add("pv_" + pv.replace(".", "_"))
    look = d.get("Look")
    if isinstance(look, dict) and look:
        s.add("look")
        if look.get("CameraModelRestriction"):
            s.add("look_camera_restricted")
        if "isAdobeAdaptive" in look:
            s.add("look_adaptive")
        if isinstance(look.get("Amount"), RawFloat):
            s.add("look_amount_float")
    elif "Look" not in d:
        s.add("no_look")
    lb = d.get("LensBlur")
    if lb == []:
        s.add("lensblur_empty")
    elif isinstance(lb, dict):
        s.add("lensblur_object")
    fl = d.get("FilterList")
    if fl == []:
        s.add("filterlist_empty")
    elif isinstance(fl, dict) and fl.get("Filters"):
        s.add("filterlist_filters")
    if isinstance(d.get("AILook"), dict) and d["AILook"]:
        s.add("ailook_object")
    if isinstance(d.get("PointColors"), list) and d["PointColors"]:
        s.add("point_colors")
    ra = d.get("RetouchAreas")
    if isinstance(ra, list) and any(isinstance(a, dict) and any(k.startswith("pm_") for k in a) for a in ra):
        s.add("retouch_areas")
    if isinstance(d.get("RemoveAreas"), list) and d["RemoveAreas"]:
        s.add("remove_areas")
    if isinstance(d.get("RetouchInfo"), (dict, list)) and d["RetouchInfo"]:
        s.add("retouch_info")
    if any(k.startswith("UprightTransform_") for k in d):
        s.add("upright_transform")
    if isinstance(d.get("DepthMapInfo"), dict) and d["DepthMapInfo"]:
        s.add("depth_map_info")
    if d.get("WhiteBalance") == "Auto" and "AutoWhiteVersion" in d:
        s.add("wb_auto")
    if isinstance(d.get("GrainSeed"), int):
        s.add("grain_seed")
    if isinstance(d.get("PerspectiveRotate"), RawFloat):
        s.add("perspective_rotate_float")
    if _has_exponent_token(d):
        s.add("number_exponent")
    for corr in d.get("MaskGroupBasedCorrections") or []:
        if not isinstance(corr, dict):
            continue
        if isinstance(corr.get("MainCurve"), list) and corr["MainCurve"]:
            s.add("local_curve")
        if any(isinstance(corr.get(k), RawFloat) for k in LEGACY_LOCAL_KEYS):
            s.add("local_legacy_float")
        for m in corr.get("CorrectionMasks") or []:
            if not isinstance(m, dict):
                continue
            shape = mask_shape(m)
            if shape:
                s.add(shape)
            crm = m.get("CorrectionRangeMask")
            if shape == "mask_range_color" and isinstance(crm, dict) and crm.get("AreaModels"):
                s.add("mask_range_color_area")
            if m.get("MaskBlendMode") == 1:
                s.add("mask_intersect" if m.get("MaskInverted") is True else "mask_subtract")
    return s


# Shapes that get a file of their own, in selection order (plan step 1, 7.2).
PRIMARY_SHAPES = [
    "mask_ai_subject",
    "mask_ai_sky",
    "mask_ai_background",
    "mask_ai_people_part_all",
    "mask_ai_people_part_person",
    "mask_ai_landscape",
    "mask_select_object_polygon",
    "mask_linear_gradient",
    "mask_radial_gradient",
    "mask_brush_aggregate",
    "mask_range_luminance",
    "mask_range_color",
    "mask_range_color_area",
    "lensblur_object",
    "filterlist_filters",
    "no_look",
]
# Shapes some file must cover; satisfied by re-picking a file where possible.
INCIDENTAL_SHAPES = [
    "look",
    "lensblur_empty",
    "filterlist_empty",
    "pv_11_0",
    "pv_15_4",
    "mask_intersect",
    "mask_subtract",
    "local_curve",
    "point_colors",
    "ailook_object",
    "look_camera_restricted",
    "look_adaptive",
    "retouch_areas",
    "remove_areas",
    "retouch_info",
    "wb_auto",
    "number_exponent",
    "upright_transform",
    "depth_map_info",
    "look_amount_float",
    "perspective_rotate_float",
    "local_legacy_float",
    "grain_seed",
]
# Shapes we only want when present; missing is reported but not an error.
OPTIONAL_SHAPES = {
    "mask_ai_people_part_all",
    "mask_ai_people_part_person",
    "mask_ai_landscape",
    "mask_range_color_area",
    "no_look",
    "mask_subtract",
    "look_adaptive",
    "remove_areas",
    "retouch_info",
    "upright_transform",
    "depth_map_info",
    "look_amount_float",
    "perspective_rotate_float",
    "local_legacy_float",
    "grain_seed",
}
WANTED_SHAPES = set(PRIMARY_SHAPES) | set(INCIDENTAL_SHAPES)

# ---------------------------------------------------------------------------
# Value rules (shared by the scrubber and the hygiene check)
# ---------------------------------------------------------------------------

# Anchored patterns end in \Z, not $ ($ also matches before a trailing
# newline), and digits are ASCII (re.ASCII), so the Rust mirror
# (crates/lrg-develop/tests/fixture_hygiene.rs, `regex` crate: $ = end of
# text, [0-9]) accepts exactly the same strings.
HEX32_RE = re.compile(r"^[0-9A-Fa-f]{32}\Z")
HEX32_ANY_RE = re.compile(r"(?<![0-9A-Fa-f])[0-9A-Fa-f]{32}(?![0-9A-Fa-f])")
GUID_RE = re.compile(r"^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\Z")
GUID_ANY_RE = re.compile(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}")
SYNTH_HEX_RE = re.compile(r"^0{20}[0-9A-Fa-f]{12}\Z")
SYNTH_GUID_RE = re.compile(r"^00000000-0000-4000-8000-[0-9A-Fa-f]{12}\Z")
# ISO dates and EXIF-style dates ("2024:05:12").
DATE_RE = re.compile(r"\d{4}[-:]\d{2}[-:]\d{2}", re.ASCII)
TABLE_KEY_RE = re.compile(r"^(Brush)?Table_[0-9A-Fa-f]{32}\Z")
KEY_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_]*\Z")
LANG_RE = re.compile(r"^(x-default|[a-z]{2,3}(-[A-Za-z0-9]{2,8})*)\Z")
# Number lists: "0.5 0.5", "2880,1920", "0/1,0/1,1920/1,2880/1", "0.70, 0.76".
NUM_TOKEN = r"[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?"
NUMERIC_TEXT_RE = re.compile(rf"^{NUM_TOKEN}([ ,/]+{NUM_TOKEN})*\Z", re.ASCII)
# Brush dab strings: "r 0.055992", "f 0.5000", "d 0.1 0.2 ...", "M 0.1 0.2".
DAB_RE = re.compile(rf"^[A-Za-z]( {NUM_TOKEN})+\Z", re.ASCII)
VERSION_RE = re.compile(r"^\d+(\.\d+){0,2}\Z", re.ASCII)
BIG_NUMBER = 1e9

HARD_TEXT_RES = [
    re.compile(r"(?i)md5p:"),
    re.compile(r"(?i)\bfile:"),
    re.compile(r"/Users/"),
    re.compile(r"/Volumes/"),
    re.compile(r"/home/"),
    re.compile(r"/private/"),
    re.compile(r"/tmp/"),
    re.compile(r"/var/"),
    re.compile(r"/Library/"),
    re.compile(r"(?i)\b[A-Z]:[\\/]"),
    re.compile(r"\\\\"),
    re.compile(r"~/"),
    re.compile(
        r"(?i)\.(cr2|cr3|crw|nef|nrw|arw|srf|sr2|raf|dng|orf|rw2|pef|srw|x3f|3fr|iiq|jpe?g|tiff?|heic|heif|psd)\b"
    ),
    re.compile(r"Table_"),
    re.compile(r"LookTable"),
    re.compile(r"RGBTable"),
]
# The dropped table keys' names; forbidden as data, but key names in the registry.
TABLE_NAME_RES = HARD_TEXT_RES[-3:]
# Files directly in --root that hold no settings, only the registry's own
# description (generated by lrg-develop's tests/registry_snapshot.rs). They
# name every key, the table keys included, so they get the text rules without
# the table-key names, the 32-hex/GUID rule, and no fixture tree rules.
REGISTRY_FILES = frozenset({"registry_snapshot.json"})

SYNTHETIC_DATE = "2000-01-01T00:00:00+00:00"
SYNTHETIC_TEXT = "Synthetic Text"
SYNTHETIC_PROFILE = "Synthetic Profile"
SYNTHETIC_GRAIN_SEED = "4000000001"  # > 2^31, so the u32 path stays covered
STUB_KEYS = frozenset({"ConvertToGrayscale", "ProcessVersion"})
# Keys whose numbers may legitimately be >= 1e9 (packed engine versions).
BIG_NUMBER_KEYS = frozenset(
    {"UprightVersion", "ModelVersion", "MinEditVersion", "MinDisplayVersion", "CompatibleVersion", "AutoWhiteVersion"}
)

CAMERA_PROFILES = frozenset(
    {
        "Adobe Standard",
        "Adobe Standard v2",
        "Camera Standard",
        "Camera Standard v2",
        "Camera Neutral",
        "Camera Neutral v2",
        "Camera Landscape",
        "Camera Landscape v2",
        "Camera Faithful",
        "Camera Faithful v2",
        "Camera Portrait",
        "Camera Portrait v2",
        "Camera Fine Detail",
        "Camera Monochrome",
    }
)
TONE_CURVE_NAMES = frozenset({"Linear", "Medium Contrast", "Strong Contrast", "Custom", "Matter Kontrast"})

# Values the scrubber writes in place of free text; the check accepts exactly these.
FIXED_VALUES: dict[str, str] = {
    "LensProfileName": "Adobe (Synthetic Lens)",
    "LensProfileFilename": "Synthetic Lens - RAW.lcp",
    "pm_clio_model_version": "synthetic-model-version",
    "Copyright": "Synthetic copyright notice",
    "Cluster": "Synthetic",
    "CameraModelRestriction": "Synthetic Camera",
}
FIXED_IN_PARENT: dict[tuple[str, str], str] = {
    ("GenAIInfo", "Name"): "Synthetic Generative Model",
    ("GenAIInfo", "SoftwareAgent"): "Synthetic Agent",
}
LOOK_ALT_FIELDS = {"Group": "Synthetic Group", "SortName": "Synthetic"}


def _one_of(values) -> Callable[[str], bool]:
    allowed = frozenset(values)
    return lambda v: v in allowed


def _match(rx: str) -> Callable[[str], bool]:
    pattern = re.compile(rx, re.ASCII)
    return lambda v: bool(pattern.fullmatch(v))


# Per-key value rules. A string under one of these keys must satisfy the rule.
STRING_RULES: dict[str, Callable[[str], bool]] = {
    "What": _match(r"^(Correction|Mask/[A-Za-z]+)$"),
    "WhiteBalance": _one_of(
        {"As Shot", "Auto", "Custom", "Daylight", "Cloudy", "Shade", "Tungsten", "Fluorescent", "Flash"}
    ),
    "ProcessVersion": VERSION_RE.match,
    "Version": VERSION_RE.match,
    "orientation": _match(r"^[A-D]{2}$"),
    "LensProfileSetup": _one_of({"LensDefaults", "Custom", "Auto"}),
    "CameraProfile": _one_of(CAMERA_PROFILES),
    "ToneCurveName": _one_of(TONE_CURVE_NAMES),
    "ToneCurveName2012": _one_of(TONE_CURVE_NAMES),
    "Method": _one_of({"poisson", "gaussian", "clone"}),
    "SourceState": _one_of({"sourceAutoComputed", "sourceSetExplicitly"}),
    "sourceState": _one_of({"sourceAutoComputed", "sourceSetExplicitly"}),
    "SpotType": _one_of({"heal", "clone", "heal_patchmatch"}),
    "spotType": _one_of({"heal", "clone", "heal_patchmatch"}),
    "fill_method": _one_of({"firefly"}),
    "pm_source_type": _one_of({"full"}),
    "CorrectionName": _match(r"^Correction \d+$"),
    "MaskName": _match(r"^Mask \d+$"),
    **{k: _one_of({v}) for k, v in FIXED_VALUES.items()},
}
STRING_RULES_IN_PARENT: dict[tuple[str, str], Callable[[str], bool]] = {
    ("Filters", "Name"): _one_of(
        {
            "Enhance",
            "Denoise",
            "Raw Details",
            "Super Resolution",
            "Dust Removal",
            "Distracting People Removal",
            "Reflection Removal",
        }
    ),
    ("Filters", "Title"): _match(r"^\$\$\$/CRaw/Filter/[A-Za-z/]+=[A-Za-z ]+$"),
    **{k: _one_of({v}) for k, v in FIXED_IN_PARENT.items()},
}


def numeric_text_ok(v: str) -> bool:
    """A number list without a lone huge integer (serial numbers, Unix times)."""
    if not NUMERIC_TEXT_RE.match(v):
        return False
    tokens = [t for t in re.split(r"[ ,/]+", v) if t]
    return not (len(tokens) == 1 and "." not in v and abs(float(v)) >= BIG_NUMBER)


def generic_string_ok(v: str) -> bool:
    """Strings that carry no identifying content, whatever the key."""
    return (
        v in ("", SYNTHETIC_TEXT, SYNTHETIC_DATE)
        or bool(SYNTH_HEX_RE.match(v) or SYNTH_GUID_RE.match(v))
        or numeric_text_ok(v)
        or bool(DAB_RE.match(v))
    )


def string_ok(v: str, key: str, parent: str) -> bool:
    """The value rule for ``key`` (under ``parent``), or the generic rule."""
    if v == SYNTHETIC_TEXT or SYNTH_HEX_RE.match(v) or SYNTH_GUID_RE.match(v):
        return True
    rule = STRING_RULES_IN_PARENT.get((parent, key)) or STRING_RULES.get(key)
    if rule is not None:
        return bool(rule(v))
    return generic_string_ok(v)


def is_dropped_key(key: str) -> bool:
    return bool(TABLE_KEY_RE.match(key)) or key.startswith(("LookTable", "RGBTable")) or key == "MaskBrushTable"


# ---------------------------------------------------------------------------
# Scrubbing
# ---------------------------------------------------------------------------


class Scrubber:
    """Stateful so that IDs map consistently across every file of one run."""

    def __init__(self, stub_look_uuid: bool) -> None:
        self.stub_look_uuid = stub_look_uuid
        self.hex_map: dict[str, str] = {}
        self.guid_map: dict[str, str] = {}
        self.report: Counter[str] = Counter()

    def hex32(self, value: str) -> str:
        key = value.upper()
        if key not in self.hex_map:
            self.hex_map[key] = "%032X" % (len(self.hex_map) + 1)
        out = self.hex_map[key]
        return out.lower() if value.islower() else out

    def guid(self, value: str) -> str:
        key = value.upper()
        if key not in self.guid_map:
            self.guid_map[key] = "00000000-0000-4000-8000-%012X" % (len(self.guid_map) + 1)
        out = self.guid_map[key]
        return out.lower() if value.islower() else out

    def _drop(self, key: str) -> None:
        if "Table_" in key:
            self.report["dropped md5 lookup-table key"] += 1
        elif key == "MaskBrushTable":
            self.report["dropped brush-table reference"] += 1
        else:
            self.report["dropped profile table key"] += 1

    def scrub_settings(self, d):
        if isinstance(d, list):  # top-level [] (empty table)
            return [self._value(v, "", "") for v in d]
        self._mask_n = 0
        self._corr_n = 0
        out = {}
        for k, v in d.items():
            if is_dropped_key(k):
                self._drop(k)
                continue
            if k == "Look" and isinstance(v, dict):
                out[k] = self._look(v)
            else:
                out[k] = self._value(v, k, "")
        return out

    def _look(self, look: dict) -> dict:
        adobe = is_adobe_look(look) and not self.stub_look_uuid
        out = {}
        for k, v in look.items():
            if is_dropped_key(k):
                self._drop(k)
                continue
            if k == "Parameters":
                out[k] = self._look_stub(v)
            elif k == "Name":
                out[k] = v if adobe else SYNTHETIC_PROFILE
            elif k == "UUID" and isinstance(v, str):
                out[k] = v if adobe else self.hex32(v)
            elif k in LOOK_ALT_FIELDS and isinstance(v, dict):
                out[k] = {lang: LOOK_ALT_FIELDS[k] for lang in v}
            else:
                out[k] = self._value(v, k, "Look")
        if not adobe and "Name" in look:
            self.report["Look name/UUID replaced (not a verified Adobe profile)"] += 1
        return out

    def _look_stub(self, params):
        """Only the two keys a reader needs; [] (JSON.lua's empty table) when neither exists."""
        self.report["Look.Parameters replaced by stub"] += 1
        params = params if isinstance(params, dict) else {}
        stub = {k: params[k] for k in sorted(STUB_KEYS) if k in params}
        if isinstance(stub.get("ProcessVersion"), str) and not VERSION_RE.match(stub["ProcessVersion"]):
            del stub["ProcessVersion"]
        if "ConvertToGrayscale" in stub and not isinstance(stub["ConvertToGrayscale"], bool):
            del stub["ConvertToGrayscale"]
        return stub or []

    def _value(self, v, key: str, parent: str):
        if isinstance(v, dict):
            out = {}
            for k, vv in v.items():
                if is_dropped_key(k):
                    self._drop(k)
                    continue
                if not KEY_RE.match(k) and not (key in LOOK_ALT_FIELDS and LANG_RE.match(k)):
                    self.report["dropped key with an unexpected name"] += 1
                    continue
                if k == "MaskName" and isinstance(vv, str):
                    self._mask_n += 1
                    out[k] = f"Mask {self._mask_n}"
                elif k == "CorrectionName" and isinstance(vv, str):
                    self._corr_n += 1
                    out[k] = f"Correction {self._corr_n}"
                else:
                    out[k] = self._value(vv, k, key)
            return out
        if isinstance(v, list):
            return [self._value(x, key, parent) for x in v]
        if isinstance(v, str):
            return self._string(v, key, parent)
        if key == "GrainSeed" and isinstance(v, int) and not isinstance(v, bool):
            self.report["GrainSeed replaced by a synthetic constant"] += 1
            return RawInt(SYNTHETIC_GRAIN_SEED)
        return v

    def _string(self, v: str, key: str, parent: str) -> str:
        if HEX32_RE.match(v):
            return self.hex32(v)
        if GUID_RE.match(v):
            return self.guid(v)
        fixed = FIXED_IN_PARENT.get((parent, key)) or FIXED_VALUES.get(key)
        if fixed is not None:
            return fixed
        if DATE_RE.search(v):
            self.report[f"date replaced ({key})"] += 1
            return SYNTHETIC_DATE
        if key == "CameraProfile" and v not in CAMERA_PROFILES:
            return "Adobe Standard"
        if key in ("ToneCurveName", "ToneCurveName2012") and v not in TONE_CURVE_NAMES:
            return "Custom"
        if string_ok(v, key, parent):
            return v
        self.report[f"free text replaced ({parent}.{key})"] += 1
        return SYNTHETIC_TEXT


# ---------------------------------------------------------------------------
# Selection
# ---------------------------------------------------------------------------


def load_rows(path: Path) -> list[tuple[str, object]]:
    """``[(develop_settings text, decoded value)]`` for every usable row."""
    rows = json.loads(path.read_text(encoding="utf-8"))
    out = []
    for r in rows:
        meta = r.get("metadata")
        meta = json.loads(meta) if isinstance(meta, str) else (meta or {})
        ds = meta.get("develop_settings")
        if not isinstance(ds, str) or not ds.strip():
            continue
        try:
            out.append((ds, loads_exact(ds)))
        except json.JSONDecodeError:
            continue
    return out


def row_cost(text: str, value) -> int:
    """Smaller is better: prefer small rows without unrelated heavy payloads."""
    cost = len(text)
    if isinstance(value, dict):
        if "BrushTable_" in text or "MaskBrushTable" in text:
            cost += 10_000_000  # its brush bitmap would be dropped, leaving a dangling reference
        for heavy in ("RetouchAreas", "RemoveAreas", "GenAIInfo", "DepthMapInfo", "UprightTransform_"):
            if heavy in text:
                cost += 500_000  # unrelated payloads: keep each fixture about one thing
        look = value.get("Look")
        if isinstance(look, dict) and not is_adobe_look(look):
            cost += 1_000_000  # prefer a verified Adobe profile; shapes that need another still find one
    return cost


def select(rows: list[tuple[str, object]]) -> tuple[list[tuple[str, int]], list[str]]:
    """Pick ``(file shape, row index)`` pairs covering every wanted shape."""
    shapes = [shapes_of(v) for _, v in rows]
    costs = [row_cost(t, v) for t, v in rows]
    taken: set[int] = set()
    picks: list[tuple[str, int]] = []
    missing: list[str] = []

    def best(pred: Callable[[int], bool]) -> int | None:
        cands = [i for i in range(len(rows)) if i not in taken and pred(i)]
        return min(cands, key=lambda i: costs[i]) if cands else None

    def covered(p: list[tuple[str, int]]) -> set[str]:
        return set().union(*(shapes[i] for _, i in p)) & WANTED_SHAPES if p else set()

    for shape in PRIMARY_SHAPES:
        i = best(lambda i, s=shape: s in shapes[i])
        if i is None:
            missing.append(shape)
            continue
        taken.add(i)
        picks.append((shape, i))

    for need in INCIDENTAL_SHAPES:
        have = covered(picks)
        if need in have:
            continue
        done = False
        # Re-pick one file within its own shape, without losing anything covered so far.
        for n, (shape, old) in enumerate(picks):
            taken.discard(old)
            others = picks[:n] + picks[n + 1 :]
            rest = covered(others)
            i = best(
                lambda i, s=shape, nd=need, hv=have, rs=rest: (
                    s in shapes[i] and nd in shapes[i] and hv <= rs | shapes[i]
                )
            )
            if i is not None:
                taken.add(i)
                picks[n] = (shape, i)
                done = True
                break
            taken.add(old)
        if not done:
            i = best(lambda i, s=need: s in shapes[i])
            if i is None:
                missing.append(need)
            else:
                taken.add(i)
                picks.append((need, i))
    return picks, missing


# ---------------------------------------------------------------------------
# Hygiene
# ---------------------------------------------------------------------------


def hard_text_problems(text: str, name: str) -> list[str]:
    return [f"{name}: forbidden text matching {rx.pattern!r}" for rx in HARD_TEXT_RES if rx.search(text)]


def check_json_tree(data, name: str, strict: bool) -> list[str]:
    """Structural rules for one fixture; ``strict`` adds the per-key value rules."""
    problems: list[str] = []

    def number(v, path: str, key: str) -> None:
        if isinstance(v, bool) or not isinstance(v, (int, float)) or abs(v) < BIG_NUMBER:
            return
        if key in BIG_NUMBER_KEYS or (key == "GrainSeed" and str(v) == SYNTHETIC_GRAIN_SEED):
            return
        problems.append(f"{name}: number >= 1e9 at {path} (per-photo value?)")

    def string(v: str, path: str, key: str, parent: str, value_rule: bool = strict) -> None:
        for h in HEX32_ANY_RE.findall(v):
            if not SYNTH_HEX_RE.match(h):
                problems.append(f"{name}: 32-hex value {h[:6]}... at {path} is not synthetic")
        for g in GUID_ANY_RE.findall(v):
            if not SYNTH_GUID_RE.match(g):
                problems.append(f"{name}: GUID {g[:8]}... at {path} is not synthetic")
        if DATE_RE.search(v) and v != SYNTHETIC_DATE:
            problems.append(f"{name}: date-like text at {path}")
        if NUMERIC_TEXT_RE.match(v) and not numeric_text_ok(v):
            problems.append(f"{name}: numeric string >= 1e9 at {path}")
        if value_rule and not string_ok(v, key, parent):
            problems.append(f"{name}: unexplained string at {path}: {v[:30]!r}")

    def look(node: dict, path: str) -> None:
        params = node.get("Parameters")
        if "Parameters" in node:
            if params != [] and not (
                isinstance(params, dict)
                and params
                and set(params) <= STUB_KEYS
                and isinstance(params.get("ConvertToGrayscale", False), bool)
                and VERSION_RE.match(str(params.get("ProcessVersion", "1")))
            ):
                problems.append(f"{name}: {path}.Parameters is not the stub (keys within {sorted(STUB_KEYS)})")
        uuid, lname = node.get("UUID"), node.get("Name")
        if isinstance(uuid, str) and uuid.upper() in ADOBE_LOOKS:
            if lname is not None and lname != ADOBE_LOOKS[uuid.upper()]:
                problems.append(f"{name}: {path}.Name does not match its Adobe UUID")
        elif isinstance(uuid, str) and not SYNTH_HEX_RE.match(uuid):
            problems.append(f"{name}: {path}.UUID is neither a verified Adobe profile nor synthetic")
        elif lname is not None and lname != SYNTHETIC_PROFILE:
            problems.append(f"{name}: {path}.Name {str(lname)[:30]!r} without a verified Adobe UUID")
        for k, v in node.items():
            if k in ("Parameters", "UUID", "Name"):
                continue
            if k in LOOK_ALT_FIELDS and isinstance(v, dict):
                for lang, text in v.items():
                    if not LANG_RE.match(lang):
                        problems.append(f"{name}: bad language tag at {path}.{k}")
                    if strict and text != LOOK_ALT_FIELDS[k]:
                        problems.append(f"{name}: unexplained string at {path}.{k}.{lang}")
                    if isinstance(text, str):
                        string(text, f"{path}.{k}.{lang}", lang, k, value_rule=False)
                continue
            walk(v, f"{path}.{k}", k, "Look")

    def walk(node, path: str, key: str, parent: str) -> None:
        if isinstance(node, dict):
            for k, v in node.items():
                p = f"{path}.{k}" if path else k
                if not KEY_RE.match(k):
                    problems.append(f"{name}: key {p[:60]!r} is not an identifier")
                if is_dropped_key(k) or k.startswith(("Table_", "BrushTable_")):
                    problems.append(f"{name}: forbidden key {p[:60]}")
                if k == "Look" and isinstance(v, dict):
                    look(v, p)
                    continue
                walk(v, p, k, key)
        elif isinstance(node, list):
            for v in node:
                walk(v, path + "[]", key, parent)
        elif isinstance(node, str):
            string(node, path, key, parent)
        else:
            number(node, path, key)

    walk(data, "", "", "")
    return problems


def check_manifest(data, name: str) -> list[str]:
    problems: list[str] = []

    def walk(node) -> None:
        if isinstance(node, dict):
            for k, v in node.items():
                walk(k)
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)
        elif isinstance(node, str):
            for h in HEX32_ANY_RE.findall(node):
                if not SYNTH_HEX_RE.match(h):
                    problems.append(f"{name}: 32-hex value {h[:6]}... is not synthetic")
            for g in GUID_ANY_RE.findall(node):
                if not SYNTH_GUID_RE.match(g):
                    problems.append(f"{name}: GUID {g[:8]}... is not synthetic")

    walk(data)
    return problems


def manifest_files(out_dir: Path) -> set[str] | None:
    """File names the manifest in ``out_dir`` owns, or ``None`` without a manifest."""
    path = out_dir / "manifest.json"
    if not path.is_file():
        return None
    data = json.loads(path.read_text(encoding="utf-8"))
    return {e["file"] for e in data.get("files", []) if isinstance(e, dict) and isinstance(e.get("file"), str)}


def hygiene_check(out_dir: Path, root: Path | None = None) -> list[str]:
    """Problems in ``out_dir`` (and every ``*.json``/``*.xmp`` below ``root``); empty means clean.

    Files the manifest owns get the strict per-key rules; ``hand_*.json`` and
    files outside ``out_dir`` get the relaxed rules. Any other ``*.json`` in
    ``out_dir`` is itself a problem.
    """
    problems: list[str] = []
    owned = manifest_files(out_dir) or set()
    files = sorted(out_dir.glob("*.json")) + sorted(out_dir.glob("*.xmp"))
    if root is not None:
        out_resolved = out_dir.resolve()
        files += sorted(
            p for p in root.rglob("*") if p.suffix in (".json", ".xmp") and p.parent.resolve() != out_resolved
        )
    for f in files:
        name = str(f)
        text = f.read_text(encoding="utf-8")
        if root is not None and f.name in REGISTRY_FILES and f.parent.resolve() == root.resolve():
            problems += [
                f"{name}: forbidden text matching {rx.pattern!r}"
                for rx in HARD_TEXT_RES
                if rx not in TABLE_NAME_RES and rx.search(text)
            ]
            try:
                problems += check_manifest(json.loads(text), name)
            except json.JSONDecodeError as e:
                problems.append(f"{name}: not valid JSON ({e})")
            continue
        problems += hard_text_problems(text, name)
        if f.suffix == ".xmp":
            for h in HEX32_ANY_RE.findall(text):
                if not SYNTH_HEX_RE.match(h) and h.upper() not in ADOBE_LOOKS:
                    problems.append(f"{name}: 32-hex value {h[:6]}... is not synthetic")
            continue
        try:
            data = json.loads(text)
        except json.JSONDecodeError as e:
            problems.append(f"{name}: not valid JSON ({e})")
            continue
        in_out_dir = f.parent.resolve() == out_dir.resolve()
        if in_out_dir and f.name == "manifest.json":
            problems += check_manifest(data, name)
            continue
        if in_out_dir and f.name not in owned and not f.name.startswith("hand_"):
            problems.append(f"{name}: not listed in manifest.json; hand-written fixtures must be named hand_*.json")
        problems += check_json_tree(data, name, strict=in_out_dir and f.name in owned)
    return problems


def report_hygiene(problems: list[str]) -> int:
    if problems:
        print(f"HYGIENE CHECK FAILED ({len(problems)} problems):", file=sys.stderr)
        for p in problems[:50]:
            print("  " + p, file=sys.stderr)
        if len(problems) > 50:
            print(f"  ... and {len(problems) - 50} more", file=sys.stderr)
        return 1
    print("hygiene check: OK")
    return 0


def self_test(out_dir: Path, root: Path) -> int:
    """Runs ``hygiene_check`` on a copy of ``root`` with one crafted file per rule.

    The same cases as ``dispatch_rules_flag_exactly_the_crafted_files`` in
    ``crates/lrg-develop/tests/fixture_hygiene.rs``: the file-level rules
    (scope, manifest ownership, the registry exemption, XMP ids) and the
    inputs the two checks once judged differently. Returns 0 when exactly the
    crafted files that should fail are flagged.
    """
    foreign_hex = "0123456789ABCDEF0123456789ABCDEF"
    masked = '{"MaskGroupBasedCorrections": [{"CorrectionMasks": [{"MaskName": "Himmel"}]}]}'
    crafted = [
        # (file relative to root, content, flagged)
        ("lua/unlisted.json", '{"Exposure2012": 0.5}', True),
        ("lua/registry_snapshot.json", '{"rows": ["LookTable"]}', True),
        ("xmp/foreign_id.xmp", f'<x crs:MaskDigest="{foreign_hex}"/>', True),
        ("xmp/adobe_look.xmp", '<x crs:UUID="B952C231111CD8E0ECCF14B86BAA7077"/>', False),
        ("lua/owned_crafted.json", masked, True),
        ("lua/hand_localised.json", masked, False),
        ("lua/stray.xmp", '<x id="md5p:0"/>', True),
        # Inputs the Python and Rust checks used to judge differently.
        ("lua/hand_newline_number.json", '{"Foo": "5000000000\\n"}', False),
        ("lua/hand_arabic_digits.json", '{"Foo": "' + "\u0665" + "\u0660" * 9 + '"}', False),
        ("lua/hand_newline_key.json", '{"Exposure\\n": 1}', True),
        ("lua/hand_newline_version.json", '{"Look": {"Parameters": {"ProcessVersion": "15.4\\n"}}}', True),
        ("lua/hand_null_name.json", '{"Look": {"Name": null}}', False),
    ]
    with tempfile.TemporaryDirectory(prefix="lrg-hygiene-self-test-") as tmp:
        copy = Path(tmp) / "develop"
        shutil.copytree(root, copy)
        out_copy = copy / out_dir.resolve().relative_to(root.resolve())
        manifest_path = out_copy / "manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["files"].append({"file": "owned_crafted.json", "covers": []})
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        (copy / "xmp").mkdir(exist_ok=True)
        for rel, content, _ in crafted:
            (copy / rel).write_text(content, encoding="utf-8")
        problems = hygiene_check(out_copy, copy)

        def flagged(rel: str) -> bool:
            return any(p.startswith(f"{copy / rel}: ") for p in problems)

        wrong = [f"{rel}: expected flagged={want}" for rel, _, want in crafted if flagged(rel) != want]
        prefixes = tuple(f"{copy / rel}: " for rel, _, _ in crafted)
        other = [p for p in problems if not p.startswith(prefixes)]
    for w in wrong:
        print("SELF-TEST: " + w, file=sys.stderr)
    for o in other:
        print("SELF-TEST: real file flagged: " + o, file=sys.stderr)
    if wrong or other:
        return 1
    print(f"hygiene self-test: OK ({len(crafted)} crafted files)")
    return 0


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

TOP_LEVEL_EMPTY_NOTE = (
    "Synthetic: JSON.lua encodes an empty getDevelopSettings() table as []; the reader must treat it "
    "as 'no settings'. No such row exists in the source dump, so this file was written by the tool."
)


def build(rows: list[tuple[str, object]], picks: list[tuple[str, int]], missing: list[str], stub_look_uuid: bool):
    """``{file name: text}`` for every generated file, including ``manifest.json``."""
    scrubber = Scrubber(stub_look_uuid=stub_look_uuid)
    files: dict[str, str] = {}
    entries = []
    covered: set[str] = set()
    for shape, i in picks:
        _, value = rows[i]
        fname = f"{shape}.json"
        files[fname] = dumps_exact(scrubber.scrub_settings(value)) + "\n"
        sh = sorted(shapes_of(value) & (WANTED_SHAPES | {"mask_select_object_brush"}))
        covered.update(sh)
        entries.append(
            {
                "file": fname,
                "primary_shape": shape,
                "covers": sh,
                "process_version": value.get("ProcessVersion") if isinstance(value, dict) else None,
                "synthetic": False,
            }
        )

    # Top-level "[]": taken from the dump if present, synthesized otherwise.
    empty_i = next((i for i, (_, v) in enumerate(rows) if v == []), None)
    files["top_level_empty.json"] = "[]\n"
    entries.append(
        {
            "file": "top_level_empty.json",
            "primary_shape": "top_level_empty",
            "covers": ["top_level_empty"],
            "process_version": None,
            "synthetic": empty_i is None,
            **({"note": TOP_LEVEL_EMPTY_NOTE} if empty_i is None else {}),
        }
    )
    covered.add("top_level_empty")

    manifest = {
        "description": (
            "Scrubbed photo:getDevelopSettings() values (as JSON.lua encodes them) for the lrg-develop "
            "Lua/JSON reader. Generated by server-rs/scripts/develop_registry/extract_fixtures.py; "
            "see ../README.md for provenance, scrubbing and the hygiene rules. Files listed here are "
            "owned by the generator and replaced on every run; hand-written fixtures are named hand_*.json."
        ),
        "look_parameters_stub": {
            "allowed_keys": sorted(STUB_KEYS),
            "meaning": "Look.Parameters reduced to these keys (or [] when neither exists); not Adobe data.",
        },
        "synthetic_values": {
            "hex32": "20 zeros + 12 hex digits, e.g. 00000000000000000000000000000001",
            "guid": "00000000-0000-4000-8000-<12 hex digits>",
            "date": SYNTHETIC_DATE,
            "GrainSeed": int(SYNTHETIC_GRAIN_SEED),
            "names": "CorrectionName 'Correction N', MaskName 'Mask N' (numbered per file)",
            "free_text": SYNTHETIC_TEXT,
        },
        "files": entries,
        "shapes_covered": sorted(covered),
        "shapes_missing": missing,
        "scrub_report": dict(sorted(scrubber.report.items())),
    }
    files["manifest.json"] = json.dumps(manifest, indent=2) + "\n"
    return files, entries, scrubber.report


def install(staging: Path, out_dir: Path, new_files: set[str]) -> None:
    """Replace the generated files in ``out_dir``; hand-written ones are never touched."""
    out_dir.mkdir(parents=True, exist_ok=True)
    old = manifest_files(out_dir) or set()
    for name in sorted(old - new_files):
        if not name.startswith("hand_") and (out_dir / name).is_file():
            (out_dir / name).unlink()
    for name in sorted(new_files):
        shutil.copy2(staging / name, out_dir / name)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Extract scrubbed getDevelopSettings() fixtures.")
    ap.add_argument("dump", nargs="?", help="training dump from dump_training.py (private, never committed)")
    ap.add_argument("--out-dir", required=True, help="fixture folder, e.g. server-rs/testdata/develop/lua")
    ap.add_argument(
        "--stub-look-uuid", action="store_true", help="also replace Adobe profile names/UUIDs with synthetic ones"
    )
    ap.add_argument("--check-only", action="store_true", help="only run the hygiene check on --out-dir")
    ap.add_argument("--root", help="with --check-only: also check every *.json/*.xmp below this folder")
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="with --root: run the hygiene check on a copy of --root with crafted files and verify what it flags",
    )
    ap.add_argument(
        "--keep-rejected", action="store_true", help="keep the private staging folder when a run is rejected"
    )
    args = ap.parse_args(argv)
    out_dir = Path(args.out_dir)

    if args.self_test:
        if not args.root:
            ap.error("--self-test needs --root")
        return self_test(out_dir, Path(args.root))
    if args.check_only:
        if manifest_files(out_dir) is None:
            print(f"no manifest.json in {out_dir}", file=sys.stderr)
            return 2
        return report_hygiene(hygiene_check(out_dir, Path(args.root) if args.root else None))
    if not args.dump:
        ap.error("the training dump is required unless --check-only is given")
    problem = output_problem(out_dir, allow_fixtures=True)
    if problem:
        print(problem, file=sys.stderr)
        return 2

    rows = load_rows(Path(args.dump))
    print(f"{len(rows)} rows with develop_settings")
    picks, missing = select(rows)
    for m in missing:
        print(f"shape not present in the dump: {m}" + ("" if m in OPTIONAL_SHAPES else "  (REQUIRED)"))
    fatal = [m for m in missing if m not in OPTIONAL_SHAPES]
    if fatal:
        print(f"FAILED: required shapes missing: {fatal}; {out_dir} left untouched", file=sys.stderr)
        return 1

    files, entries, report = build(rows, picks, missing, args.stub_look_uuid)
    staging = Path(tempfile.mkdtemp(prefix="lrg-fixtures-"))  # mode 0700
    try:
        for name, text in files.items():
            (staging / name).write_text(text, encoding="utf-8")
        for e in entries:
            print(f"  {e['file']:<36} PV {e['process_version']!s:<5} covers {', '.join(e['covers'])}")
        print("scrub:", dict(sorted(report.items())))
        if report_hygiene(hygiene_check(staging)):
            if args.keep_rejected:
                print(f"rejected output kept in {staging} (private; delete it when done)", file=sys.stderr)
            print(f"{out_dir} left untouched", file=sys.stderr)
            return 1
        install(staging, out_dir, set(files))
    finally:
        if not args.keep_rejected:
            shutil.rmtree(staging, ignore_errors=True)
    print(f"{len(files)} files written to {out_dir}")
    return report_hygiene(hygiene_check(out_dir))


if __name__ == "__main__":
    sys.exit(main())
