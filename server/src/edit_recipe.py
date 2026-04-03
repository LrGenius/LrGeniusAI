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
            "additionalProperties": False,
        }
    return {
        "type": "object",
        "properties": properties,
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
            "additionalProperties": False,
        }
    properties["global"] = {
        "type": "object",
        "properties": {
            "hue": _number_schema(0.0, 360.0),
            "saturation": _number_schema(0.0, 100.0),
        },
        "additionalProperties": False,
    }
    properties["blending"] = _number_schema(0.0, 100.0)
    properties["balance"] = _number_schema(-100.0, 100.0)
    return {
        "type": "object",
        "properties": properties,
        "additionalProperties": False,
    }


def _build_global_schema() -> Dict[str, Any]:
    properties: Dict[str, Any] = {
        field_name: _number_schema(bounds["min"], bounds["max"])
        for field_name, bounds in GLOBAL_FIELD_RANGES.items()
    }
    properties["hsl"] = _build_hsl_schema()
    properties["color_grading"] = _build_color_grading_schema()
    properties["lens_corrections"] = {
        "type": "object",
        "properties": {
            "enable_profile_corrections": {"type": "boolean"},
            "remove_chromatic_aberration": {"type": "boolean"},
        },
        "additionalProperties": False,
    }
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
                "additionalProperties": False,
            },
        },
        "additionalProperties": False,
    }
    return {
        "type": "object",
        "properties": properties,
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
                "additionalProperties": False,
            },
        },
        "required": ["kind", "adjustments"],
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

    return deepcopy(schema)


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


def _normalize_global_settings(global_settings: Any) -> Dict[str, Any]:
    if not isinstance(global_settings, dict):
        return {}

    normalized: Dict[str, Any] = {}
    for field_name, bounds in GLOBAL_FIELD_RANGES.items():
        if field_name not in global_settings:
            continue
        clamped = _clamp_number(global_settings.get(field_name), bounds["min"], bounds["max"])
        if clamped is not None:
            normalized[field_name] = clamped

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
                normalized_points = _normalize_point_curve_points(point_curve.get(channel), 0.0, 255.0)
                if normalized_points:
                    normalized_point_curve[channel] = normalized_points
            if normalized_point_curve:
                normalized_curve["point_curve"] = normalized_point_curve

        extended_point_curve = tone_curve.get("extended_point_curve")
        if isinstance(extended_point_curve, dict):
            normalized_extended_curve: Dict[str, List[int]] = {}
            for channel in ("master", "red", "green", "blue"):
                normalized_points = _normalize_point_curve_points(extended_point_curve.get(channel), 0.0, 4096.0)
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
                clamped = _clamp_number(region_data.get(key), bounds["min"], bounds["max"])
                if clamped is not None:
                    normalized_region[key] = clamped
            if normalized_region:
                normalized_grading[region] = normalized_region

        global_region = color_grading.get("global")
        if isinstance(global_region, dict):
            normalized_global_region: Dict[str, float] = {}
            for key, bounds in {"hue": {"min": 0.0, "max": 360.0}, "saturation": {"min": 0.0, "max": 100.0}}.items():
                clamped = _clamp_number(global_region.get(key), bounds["min"], bounds["max"])
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

    lens_corrections = global_settings.get("lens_corrections")
    if isinstance(lens_corrections, dict):
        normalized_lens: Dict[str, bool] = {}
        for key in ("enable_profile_corrections", "remove_chromatic_aberration"):
            value = lens_corrections.get(key)
            if isinstance(value, bool):
                normalized_lens[key] = value
        if normalized_lens:
            normalized["lens_corrections"] = normalized_lens

    return normalized


def _normalize_point_curve_points(value: Any, minimum: float, maximum: float) -> List[int]:
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
            warnings.append(f"Ignored mask #{index + 1}: unsupported kind '{kind or 'unknown'}'.")
            continue

        raw_adjustments = mask.get("adjustments")
        if not isinstance(raw_adjustments, dict):
            warnings.append(f"Ignored mask '{kind}': adjustments were missing.")
            continue

        normalized_adjustments: Dict[str, float] = {}
        for field_name, bounds in MASK_ADJUSTMENT_RANGES.items():
            if field_name not in raw_adjustments:
                continue
            clamped = _clamp_number(raw_adjustments.get(field_name), bounds["min"], bounds["max"])
            if clamped is not None:
                normalized_adjustments[field_name] = clamped

        if not normalized_adjustments:
            warnings.append(f"Ignored mask '{kind}': no supported adjustments were returned.")
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
        "global": _normalize_global_settings(parsed_data.get("global")),
        "masks": _normalize_masks(parsed_data.get("masks"), warnings),
        "warnings": warnings,
    }

    if not normalized["summary"]:
        normalized["summary"] = "AI-generated Lightroom edit recipe"
    return normalized
