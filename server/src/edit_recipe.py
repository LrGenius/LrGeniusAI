"""
Structured Lightroom edit recipe helpers.

This module defines the canonical edit contract used between the LLM backend
and the Lightroom plugin. The schema stays provider-agnostic while the plugin
maps canonical fields onto Lightroom-specific develop keys.
"""

from __future__ import annotations

from copy import deepcopy
from typing import Any, Dict, List


GLOBAL_FIELD_RANGES: Dict[str, Dict[str, float]] = {
    "exposure": {"min": -5.0, "max": 5.0},
    "contrast": {"min": -100.0, "max": 100.0},
    "highlights": {"min": -100.0, "max": 100.0},
    "shadows": {"min": -100.0, "max": 100.0},
    "whites": {"min": -100.0, "max": 100.0},
    "blacks": {"min": -100.0, "max": 100.0},
    "temperature": {"min": 2000.0, "max": 50000.0},
    "tint": {"min": -150.0, "max": 150.0},
    "texture": {"min": -100.0, "max": 100.0},
    "clarity": {"min": -100.0, "max": 100.0},
    "dehaze": {"min": -100.0, "max": 100.0},
    "vibrance": {"min": -100.0, "max": 100.0},
    "saturation": {"min": -100.0, "max": 100.0},
    "sharpening": {"min": 0.0, "max": 150.0},
    "sharpen_radius": {"min": 0.5, "max": 3.0},
    "sharpen_detail": {"min": 0.0, "max": 100.0},
    "sharpen_masking": {"min": 0.0, "max": 100.0},
    "noise_reduction": {"min": 0.0, "max": 100.0},
    "noise_reduction_detail": {"min": 0.0, "max": 100.0},
    "noise_reduction_contrast": {"min": 0.0, "max": 100.0},
    "color_noise_reduction": {"min": 0.0, "max": 100.0},
    "color_noise_reduction_detail": {"min": 0.0, "max": 100.0},
    "color_noise_reduction_smoothness": {"min": 0.0, "max": 100.0},
    "vignette": {"min": -100.0, "max": 100.0},
    "vignette_midpoint": {"min": 0.0, "max": 100.0},
    "vignette_roundness": {"min": -100.0, "max": 100.0},
    "vignette_feather": {"min": 0.0, "max": 100.0},
    "vignette_highlights": {"min": 0.0, "max": 100.0},
    "grain": {"min": 0.0, "max": 100.0},
    "grain_size": {"min": 0.0, "max": 100.0},
    "grain_roughness": {"min": 0.0, "max": 100.0},
}

MASK_ADJUSTMENT_RANGES: Dict[str, Dict[str, float]] = {
    "exposure": {"min": -5.0, "max": 5.0},
    "contrast": {"min": -100.0, "max": 100.0},
    "highlights": {"min": -100.0, "max": 100.0},
    "shadows": {"min": -100.0, "max": 100.0},
    "whites": {"min": -100.0, "max": 100.0},
    "blacks": {"min": -100.0, "max": 100.0},
    "temperature": {"min": -100.0, "max": 100.0},
    "tint": {"min": -100.0, "max": 100.0},
    "texture": {"min": -100.0, "max": 100.0},
    "clarity": {"min": -100.0, "max": 100.0},
    "dehaze": {"min": -100.0, "max": 100.0},
    "saturation": {"min": -100.0, "max": 100.0},
    "sharpness": {"min": -100.0, "max": 100.0},
    "noise": {"min": -100.0, "max": 100.0},
    "moire": {"min": -100.0, "max": 100.0},
}

HSL_CHANNELS = ("red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta")
COLOR_GRADING_RANGES = {
    "hue": {"min": 0.0, "max": 360.0},
    "saturation": {"min": 0.0, "max": 100.0},
    "luminance": {"min": -100.0, "max": 100.0},
}
MASK_KINDS = ("subject", "sky", "background")


def _number_schema(minimum: float, maximum: float) -> Dict[str, Any]:
    return {
        "type": "number",
        "minimum": minimum,
        "maximum": maximum,
    }


def _integer_schema(minimum: int, maximum: int) -> Dict[str, Any]:
    return {
        "type": "integer",
        "minimum": minimum,
        "maximum": maximum,
    }


def _build_hsl_schema() -> Dict[str, Any]:
    properties: Dict[str, Any] = {}
    for channel in HSL_CHANNELS:
        properties[channel] = {
            "type": "object",
            "properties": {
                "hue": _number_schema(-100.0, 100.0),
                "saturation": _number_schema(-100.0, 100.0),
                "luminance": _number_schema(-100.0, 100.0),
            },
            "required": ["hue", "saturation", "luminance"],
            "additionalProperties": False,
        }
    return {
        "type": "object",
        "properties": properties,
        "required": list(HSL_CHANNELS),
        "additionalProperties": False,
    }


def _build_color_grading_schema() -> Dict[str, Any]:
    properties: Dict[str, Any] = {}
    for region in ("shadows", "midtones", "highlights"):
        properties[region] = {
            "type": "object",
            "properties": {
                key: _number_schema(bounds["min"], bounds["max"])
                for key, bounds in COLOR_GRADING_RANGES.items()
            },
            "required": list(COLOR_GRADING_RANGES.keys()),
            "additionalProperties": False,
        }
    properties["global"] = {
        "type": "object",
        "properties": {
            "hue": _number_schema(0.0, 360.0),
            "saturation": _number_schema(0.0, 100.0),
        },
        "required": ["hue", "saturation"],
        "additionalProperties": False,
    }
    properties["blending"] = _number_schema(0.0, 100.0)
    properties["balance"] = _number_schema(-100.0, 100.0)
    required = ["shadows", "midtones", "highlights", "global", "blending", "balance"]
    return {
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": False,
    }


def _build_global_schema() -> Dict[str, Any]:
    properties: Dict[str, Any] = {
        field_name: _number_schema(bounds["min"], bounds["max"])
        for field_name, bounds in GLOBAL_FIELD_RANGES.items()
    }
    properties["hsl"] = _build_hsl_schema()
    properties["white_balance"] = {
        "type": "object",
        "properties": {
            "temperature": _number_schema(2000.0, 50000.0),
            "tint": _number_schema(-150.0, 150.0),
        },
        "required": ["temperature", "tint"],
        "additionalProperties": False,
    }
    properties["crop"] = {
        "type": "object",
        "properties": {
            "left": _number_schema(0.0, 1.0),
            "right": _number_schema(0.0, 1.0),
            "top": _number_schema(0.0, 1.0),
            "bottom": _number_schema(0.0, 1.0),
            "angle": _number_schema(-45.0, 45.0),
        },
        "required": ["left", "right", "top", "bottom", "angle"],
        "additionalProperties": False,
    }
    properties["color_grading"] = _build_color_grading_schema()
    properties["tone_curve"] = {
        "type": "object",
        "properties": {
            "highlights": _number_schema(-100.0, 100.0),
            "lights": _number_schema(-100.0, 100.0),
            "darks": _number_schema(-100.0, 100.0),
            "shadows": _number_schema(-100.0, 100.0),
            "shadow_split": _number_schema(0.0, 100.0),
            "midtone_split": _number_schema(0.0, 100.0),
            "highlight_split": _number_schema(0.0, 100.0),
            "point_curve": {
                "type": "object",
                "properties": {
                    "master": {
                        "type": "array",
                        "items": _number_schema(0.0, 255.0),
                    },
                    "red": {
                        "type": "array",
                        "items": _number_schema(0.0, 255.0),
                    },
                    "green": {
                        "type": "array",
                        "items": _number_schema(0.0, 255.0),
                    },
                    "blue": {
                        "type": "array",
                        "items": _number_schema(0.0, 255.0),
                    },
                },
                "required": ["master", "red", "green", "blue"],
                "additionalProperties": False,
            },
            "extended_point_curve": {
                "type": "object",
                "properties": {
                    "master": {
                        "type": "array",
                        "items": _number_schema(0.0, 4096.0),
                    },
                    "red": {
                        "type": "array",
                        "items": _number_schema(0.0, 4096.0),
                    },
                    "green": {
                        "type": "array",
                        "items": _number_schema(0.0, 4096.0),
                    },
                    "blue": {
                        "type": "array",
                        "items": _number_schema(0.0, 4096.0),
                    },
                },
                "required": ["master", "red", "green", "blue"],
                "additionalProperties": False,
            },
        },
        "required": [
            "highlights",
            "lights",
            "darks",
            "shadows",
            "shadow_split",
            "midtone_split",
            "highlight_split",
            "point_curve",
            "extended_point_curve",
        ],
        "additionalProperties": False,
    }
    required = list(GLOBAL_FIELD_RANGES.keys()) + [
        "hsl",
        "white_balance",
        "crop",
        "color_grading",
        "tone_curve",
    ]
    return {
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": False,
    }


def _build_mask_schema() -> Dict[str, Any]:
    adjustment_properties = {
        field_name: _number_schema(bounds["min"], bounds["max"])
        for field_name, bounds in MASK_ADJUSTMENT_RANGES.items()
    }
    return {
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": list(MASK_KINDS),
            },
            "name": {"type": "string"},
            "invert": {"type": "boolean"},
            "adjustments": {
                "type": "object",
                "properties": adjustment_properties,
                "required": list(MASK_ADJUSTMENT_RANGES.keys()),
                "additionalProperties": False,
            },
        },
        "required": ["kind", "name", "invert", "adjustments"],
        "additionalProperties": False,
    }


OPENAI_EDIT_RECIPE_SCHEMA: Dict[str, Any] = {
    "type": "object",
    "properties": {
        "summary": {"type": "string"},
        "global": _build_global_schema(),
        "masks": {
            "type": "array",
            "items": _build_mask_schema(),
        },
        "warnings": {
            "type": "array",
            "items": {"type": "string"},
        },
    },
    "required": ["summary", "global", "masks", "warnings"],
    "additionalProperties": False,
}


def _convert_openai_schema_to_gemini(schema: Dict[str, Any]) -> Dict[str, Any]:
    schema_type = schema.get("type")
    if schema_type == "object":
        result: Dict[str, Any] = {
            "type": "OBJECT",
            "properties": {},
        }
        if "required" in schema:
            result["required"] = list(schema.get("required") or [])
        for key, value in schema.get("properties", {}).items():
            result["properties"][key] = _convert_openai_schema_to_gemini(value)
        return result

    if schema_type == "array":
        return {
            "type": "ARRAY",
            "items": _convert_openai_schema_to_gemini(schema["items"]),
        }

    if schema_type == "string":
        result = {"type": "STRING"}
        if "enum" in schema:
            result["enum"] = list(schema["enum"])
        return result

    if schema_type == "boolean":
        return {"type": "BOOLEAN"}

    if schema_type == "integer":
        result = {"type": "INTEGER"}
        if "minimum" in schema:
            result["minimum"] = schema["minimum"]
        if "maximum" in schema:
            result["maximum"] = schema["maximum"]
        return result

    if schema_type == "number":
        result = {"type": "NUMBER"}
        if "minimum" in schema:
            result["minimum"] = schema["minimum"]
        if "maximum" in schema:
            result["maximum"] = schema["maximum"]
        return result

    # Standard fallback - but Gemini doesn't support additionalProperties
    result = deepcopy(schema)
    if isinstance(result, dict):
        result.pop("additionalProperties", None)
    return result


GEMINI_EDIT_RECIPE_SCHEMA = _convert_openai_schema_to_gemini(OPENAI_EDIT_RECIPE_SCHEMA)


def _clamp_number(value: Any, minimum: float, maximum: float) -> float | None:
    try:
        numeric = float(value)
    except (TypeError, ValueError):
        return None
    if numeric < minimum:
        numeric = minimum
    if numeric > maximum:
        numeric = maximum
    return round(numeric, 4)


def _normalize_text(value: Any) -> str:
    if not isinstance(value, str):
        return ""
    return value.strip()


def _normalize_warning_list(value: Any) -> List[str]:
    if not isinstance(value, list):
        return []
    warnings: List[str] = []
    for item in value:
        text = _normalize_text(item)
        if text:
            warnings.append(text)
    return warnings


def _normalize_crop_settings(
    crop: Any, warnings: List[str] | None = None
) -> Dict[str, float]:
    if not isinstance(crop, dict):
        return {}

    normalized_crop: Dict[str, float] = {}

    # Canonical schema: left/right/top/bottom(+angle)
    has_canonical_edges = False
    for key in ("left", "right", "top", "bottom"):
        clamped = _clamp_number(crop.get(key), 0.0, 1.0)
        if clamped is not None:
            normalized_crop[key] = clamped
            has_canonical_edges = True

    # Compatibility shape frequently produced by LLMs:
    # x/y/width/height (+rotation) where x/y is top-left.
    if not has_canonical_edges:
        x = _clamp_number(crop.get("x"), 0.0, 1.0)
        y = _clamp_number(crop.get("y"), 0.0, 1.0)
        width = _clamp_number(crop.get("width"), 0.0, 1.0)
        height = _clamp_number(crop.get("height"), 0.0, 1.0)
        if x is not None and y is not None and width is not None and height is not None:
            normalized_crop["left"] = x
            normalized_crop["top"] = y
            normalized_crop["right"] = min(1.0, round(x + width, 4))
            normalized_crop["bottom"] = min(1.0, round(y + height, 4))

    # Accept both angle and rotation synonyms.
    clamped_angle = _clamp_number(crop.get("angle"), -45.0, 45.0)
    if clamped_angle is None:
        clamped_angle = _clamp_number(crop.get("rotation"), -45.0, 45.0)
    if clamped_angle is not None:
        normalized_crop["angle"] = clamped_angle

    # Require a valid rectangle before returning crop edges.
    left = normalized_crop.get("left")
    right = normalized_crop.get("right")
    top = normalized_crop.get("top")
    bottom = normalized_crop.get("bottom")
    if left is not None and right is not None and left >= right:
        if warnings is not None:
            warnings.append(
                "Ignored crop: left edge was not smaller than right edge after normalization."
            )
        normalized_crop.pop("left", None)
        normalized_crop.pop("right", None)
    if top is not None and bottom is not None and top >= bottom:
        if warnings is not None:
            warnings.append(
                "Ignored crop: top edge was not smaller than bottom edge after normalization."
            )
        normalized_crop.pop("top", None)
        normalized_crop.pop("bottom", None)

    # If only one side of an edge pair exists, drop it to avoid invalid partial crops.
    if ("left" in normalized_crop) != ("right" in normalized_crop):
        normalized_crop.pop("left", None)
        normalized_crop.pop("right", None)
    if ("top" in normalized_crop) != ("bottom" in normalized_crop):
        normalized_crop.pop("top", None)
        normalized_crop.pop("bottom", None)

    return normalized_crop


def _normalize_global_settings(
    global_settings: Any, warnings: List[str] | None = None
) -> Dict[str, Any]:
    if not isinstance(global_settings, dict):
        return {}

    normalized: Dict[str, Any] = {}
    for field_name, bounds in GLOBAL_FIELD_RANGES.items():
        if field_name not in global_settings:
            continue
        clamped = _clamp_number(
            global_settings.get(field_name), bounds["min"], bounds["max"]
        )
        if clamped is not None:
            normalized[field_name] = clamped

    white_balance = global_settings.get("white_balance")
    if isinstance(white_balance, dict):
        if "temperature" not in normalized:
            clamped_temp = _clamp_number(
                white_balance.get("temperature"), 2000.0, 50000.0
            )
            if clamped_temp is not None:
                normalized["temperature"] = clamped_temp
        if "tint" not in normalized:
            clamped_tint = _clamp_number(white_balance.get("tint"), -150.0, 150.0)
            if clamped_tint is not None:
                normalized["tint"] = clamped_tint

    crop = global_settings.get("crop")
    if isinstance(crop, dict):
        normalized_crop = _normalize_crop_settings(crop, warnings=warnings)
        if normalized_crop:
            normalized["crop"] = normalized_crop
        elif warnings is not None:
            warnings.append(
                "Ignored crop: no supported crop fields were returned by the model."
            )

    tone_curve = global_settings.get("tone_curve")
    if isinstance(tone_curve, dict):
        normalized_curve: Dict[str, Any] = {}
        for key in ("highlights", "lights", "darks", "shadows"):
            clamped = _clamp_number(tone_curve.get(key), -100.0, 100.0)
            if clamped is not None:
                normalized_curve[key] = clamped
        for key in ("shadow_split", "midtone_split", "highlight_split"):
            clamped = _clamp_number(tone_curve.get(key), 0.0, 100.0)
            if clamped is not None:
                normalized_curve[key] = clamped

        point_curve = tone_curve.get("point_curve")
        if isinstance(point_curve, dict):
            normalized_point_curve: Dict[str, List[int]] = {}
            for channel in ("master", "red", "green", "blue"):
                normalized_points = _normalize_point_curve_points(
                    point_curve.get(channel), 0.0, 255.0
                )
                if normalized_points:
                    normalized_point_curve[channel] = normalized_points
            if normalized_point_curve:
                normalized_curve["point_curve"] = normalized_point_curve

        extended_point_curve = tone_curve.get("extended_point_curve")
        if isinstance(extended_point_curve, dict):
            normalized_extended_curve: Dict[str, List[int]] = {}
            for channel in ("master", "red", "green", "blue"):
                normalized_points = _normalize_point_curve_points(
                    extended_point_curve.get(channel), 0.0, 4096.0
                )
                if normalized_points:
                    normalized_extended_curve[channel] = normalized_points
            if normalized_extended_curve:
                normalized_curve["extended_point_curve"] = normalized_extended_curve
        if normalized_curve:
            normalized["tone_curve"] = normalized_curve

    hsl = global_settings.get("hsl")
    if isinstance(hsl, dict):
        normalized_hsl: Dict[str, Dict[str, float]] = {}
        for channel in HSL_CHANNELS:
            channel_data = hsl.get(channel)
            if not isinstance(channel_data, dict):
                continue
            normalized_channel: Dict[str, float] = {}
            for key in ("hue", "saturation", "luminance"):
                clamped = _clamp_number(channel_data.get(key), -100.0, 100.0)
                if clamped is not None:
                    normalized_channel[key] = clamped
            if normalized_channel:
                normalized_hsl[channel] = normalized_channel
        if normalized_hsl:
            normalized["hsl"] = normalized_hsl

    color_grading = global_settings.get("color_grading")
    if isinstance(color_grading, dict):
        normalized_grading: Dict[str, Any] = {}
        for region in ("shadows", "midtones", "highlights"):
            region_data = color_grading.get(region)
            if not isinstance(region_data, dict):
                continue
            normalized_region: Dict[str, float] = {}
            for key, bounds in COLOR_GRADING_RANGES.items():
                clamped = _clamp_number(
                    region_data.get(key), bounds["min"], bounds["max"]
                )
                if clamped is not None:
                    normalized_region[key] = clamped
            if normalized_region:
                normalized_grading[region] = normalized_region

        global_region = color_grading.get("global")
        if isinstance(global_region, dict):
            normalized_global_region: Dict[str, float] = {}
            for key, bounds in {
                "hue": {"min": 0.0, "max": 360.0},
                "saturation": {"min": 0.0, "max": 100.0},
            }.items():
                clamped = _clamp_number(
                    global_region.get(key), bounds["min"], bounds["max"]
                )
                if clamped is not None:
                    normalized_global_region[key] = clamped
            if normalized_global_region:
                normalized_grading["global"] = normalized_global_region

        blending = _clamp_number(color_grading.get("blending"), 0.0, 100.0)
        if blending is not None:
            normalized_grading["blending"] = blending
        balance = _clamp_number(color_grading.get("balance"), -100.0, 100.0)
        if balance is not None:
            normalized_grading["balance"] = balance
        if normalized_grading:
            normalized["color_grading"] = normalized_grading

    return normalized


def _normalize_point_curve_points(
    value: Any, minimum: float, maximum: float
) -> List[int]:
    if not isinstance(value, list) or len(value) < 2:
        return []

    # Accept either flat [x1, y1, x2, y2, ...] or [{x,y}, ...] / [[x,y], ...].
    points: List[int] = []
    if value and isinstance(value[0], (dict, list, tuple)):
        for item in value:
            x_val = None
            y_val = None
            if isinstance(item, dict):
                x_val = item.get("x")
                y_val = item.get("y")
            elif isinstance(item, (list, tuple)) and len(item) >= 2:
                x_val = item[0]
                y_val = item[1]
            x = _clamp_number(x_val, minimum, maximum)
            y = _clamp_number(y_val, minimum, maximum)
            if x is None or y is None:
                continue
            points.append(int(round(x)))
            points.append(int(round(y)))
    else:
        for numeric in value:
            clamped = _clamp_number(numeric, minimum, maximum)
            if clamped is None:
                continue
            points.append(int(round(clamped)))

    # Need at least two points and an even number of entries.
    if len(points) % 2 == 1:
        points = points[:-1]
    if len(points) < 4:
        return []
    return points


def _normalize_masks(masks: Any, warnings: List[str]) -> List[Dict[str, Any]]:
    if not isinstance(masks, list):
        return []

    normalized_masks: List[Dict[str, Any]] = []
    for index, mask in enumerate(masks):
        if not isinstance(mask, dict):
            warnings.append(f"Ignored mask #{index + 1}: expected an object.")
            continue

        kind = _normalize_text(mask.get("kind")).lower()
        if kind not in MASK_KINDS:
            warnings.append(
                f"Ignored mask #{index + 1}: unsupported kind '{kind or 'unknown'}'."
            )
            continue

        raw_adjustments = mask.get("adjustments")
        if not isinstance(raw_adjustments, dict):
            warnings.append(f"Ignored mask '{kind}': adjustments were missing.")
            continue

        normalized_adjustments: Dict[str, float] = {}
        for field_name, bounds in MASK_ADJUSTMENT_RANGES.items():
            if field_name not in raw_adjustments:
                continue
            clamped = _clamp_number(
                raw_adjustments.get(field_name), bounds["min"], bounds["max"]
            )
            if clamped is not None:
                normalized_adjustments[field_name] = clamped

        if not normalized_adjustments:
            warnings.append(
                f"Ignored mask '{kind}': no supported adjustments were returned."
            )
            continue

        normalized_mask = {
            "kind": kind,
            "adjustments": normalized_adjustments,
        }
        name = _normalize_text(mask.get("name"))
        if name:
            normalized_mask["name"] = name
        if isinstance(mask.get("invert"), bool):
            normalized_mask["invert"] = mask["invert"]
        normalized_masks.append(normalized_mask)

    return normalized_masks


def normalize_edit_recipe(parsed_data: Any) -> Dict[str, Any]:
    warnings: List[str] = []
    if not isinstance(parsed_data, dict):
        return {
            "summary": "",
            "global": {},
            "masks": [],
            "warnings": ["LLM returned an invalid edit recipe payload."],
        }

    warnings.extend(_normalize_warning_list(parsed_data.get("warnings")))
    normalized = {
        "summary": _normalize_text(parsed_data.get("summary")),
        "global": _normalize_global_settings(parsed_data.get("global"), warnings),
        "masks": _normalize_masks(parsed_data.get("masks"), warnings),
        "warnings": warnings,
    }

    if not normalized["summary"]:
        normalized["summary"] = "AI-generated Lightroom edit recipe"
    return normalized


def filter_edit_recipe_by_controls(
    recipe: Dict[str, Any], controls: Dict[str, bool]
) -> Dict[str, Any]:
    if not isinstance(recipe, dict):
        return recipe

    filtered = deepcopy(recipe)
    global_settings = filtered.get("global")
    if not isinstance(global_settings, dict):
        global_settings = {}
        filtered["global"] = global_settings

    def _drop(keys: List[str]) -> None:
        for key in keys:
            global_settings.pop(key, None)

    if not controls.get("adjust_white_balance", True):
        _drop(["temperature", "tint", "white_balance"])
    if not controls.get("adjust_basic_tone", True):
        _drop(["exposure", "contrast", "highlights", "shadows", "whites", "blacks"])
    if not controls.get("adjust_presence", True):
        _drop(["texture", "clarity", "dehaze"])
    if not controls.get("adjust_color_mix", True):
        _drop(["vibrance", "saturation", "hsl"])
    if not controls.get("do_color_grading", True):
        _drop(["color_grading"])
    if not controls.get("use_tone_curve", True):
        _drop(["tone_curve"])
    else:
        if not controls.get("use_point_curve", True):
            tone_curve = global_settings.get("tone_curve")
            if isinstance(tone_curve, dict):
                tone_curve.pop("point_curve", None)
                tone_curve.pop("extended_point_curve", None)
                if not tone_curve:
                    global_settings.pop("tone_curve", None)
    if not controls.get("adjust_detail", True):
        _drop(
            [
                "sharpening",
                "sharpen_radius",
                "sharpen_detail",
                "sharpen_masking",
                "noise_reduction",
                "noise_reduction_detail",
                "noise_reduction_contrast",
                "color_noise_reduction",
                "color_noise_reduction_detail",
                "color_noise_reduction_smoothness",
            ]
        )
    if not controls.get("adjust_effects", True):
        _drop(
            [
                "vignette",
                "vignette_midpoint",
                "vignette_roundness",
                "vignette_feather",
                "vignette_highlights",
                "grain",
                "grain_size",
                "grain_roughness",
            ]
        )
    if not controls.get("adjust_lens_corrections", True):
        _drop(["lens_corrections"])
    composition_mode = str(controls.get("composition_mode", "subtle")).lower()
    if composition_mode == "none" or not controls.get("allow_auto_crop", True):
        _drop(["crop"])

    masks = filtered.get("masks")
    if not controls.get("include_masks", True):
        filtered["masks"] = []
    elif isinstance(masks, list):
        allowed_mask_adjustments = set(MASK_ADJUSTMENT_RANGES.keys())
        if not controls.get("adjust_white_balance", True):
            allowed_mask_adjustments -= {"temperature", "tint"}
        if not controls.get("adjust_basic_tone", True):
            allowed_mask_adjustments -= {
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
            }
        if not controls.get("adjust_presence", True):
            allowed_mask_adjustments -= {"texture", "clarity", "dehaze"}
        if not controls.get("adjust_color_mix", True):
            allowed_mask_adjustments -= {"saturation"}
        if not controls.get("adjust_detail", True):
            allowed_mask_adjustments -= {"sharpness", "noise", "moire"}

        kept_masks: List[Dict[str, Any]] = []
        for mask in masks:
            if not isinstance(mask, dict):
                continue
            adjustments = mask.get("adjustments")
            if not isinstance(adjustments, dict):
                continue
            kept_adjustments = {
                k: v for k, v in adjustments.items() if k in allowed_mask_adjustments
            }
            if not kept_adjustments:
                continue
            next_mask = deepcopy(mask)
            next_mask["adjustments"] = kept_adjustments
            kept_masks.append(next_mask)
        filtered["masks"] = kept_masks

    return filtered
