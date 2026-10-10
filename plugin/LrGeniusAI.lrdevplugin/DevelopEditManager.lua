DevelopEditManager = {}

local GLOBAL_KEY_MAP = {
	exposure = "Exposure2012",
	contrast = "Contrast2012",
	highlights = "Highlights2012",
	shadows = "Shadows2012",
	whites = "Whites2012",
	blacks = "Blacks2012",
	-- No temperature/tint here: which Lightroom keys carry a white balance
	-- depends on the photo (see buildWhiteBalanceSettings), so a fixed name
	-- cannot be right for every file.
	texture = "Texture",
	clarity = "Clarity2012",
	dehaze = "Dehaze",
	vibrance = "Vibrance",
	saturation = "Saturation",
	sharpening = "Sharpness",
	sharpen_radius = "SharpenRadius",
	sharpen_detail = "SharpenDetail",
	sharpen_masking = "SharpenEdgeMasking",
	noise_reduction = "LuminanceSmoothing",
	noise_reduction_detail = "LuminanceNoiseReductionDetail",
	noise_reduction_contrast = "LuminanceNoiseReductionContrast",
	color_noise_reduction = "ColorNoiseReduction",
	color_noise_reduction_detail = "ColorNoiseReductionDetail",
	color_noise_reduction_smoothness = "ColorNoiseReductionSmoothness",
	vignette = "PostCropVignetteAmount",
	vignette_midpoint = "PostCropVignetteMidpoint",
	vignette_roundness = "PostCropVignetteRoundness",
	vignette_feather = "PostCropVignetteFeather",
	vignette_highlights = "PostCropVignetteHighlightContrast",
	grain = "GrainAmount",
	grain_size = "GrainSize",
	grain_roughness = "GrainFrequency",
}

local MASK_KEY_CANDIDATES = {
	exposure = { "local_Exposure", "Exposure2012", "Exposure" },
	contrast = { "local_Contrast", "Contrast2012", "Contrast" },
	highlights = { "local_Highlights", "Highlights2012", "Highlights" },
	shadows = { "local_Shadows", "Shadows2012", "Shadows" },
	whites = { "local_Whites", "Whites2012", "Whites" },
	blacks = { "local_Blacks", "Blacks2012", "Blacks" },
	temperature = { "local_Temperature", "Temperature" },
	tint = { "local_Tint", "Tint" },
	texture = { "local_Texture", "Texture" },
	clarity = { "local_Clarity", "Clarity2012", "Clarity" },
	dehaze = { "local_Dehaze", "Dehaze" },
	saturation = { "local_Saturation", "Saturation" },
	sharpness = { "local_Sharpness", "Sharpness" },
	noise = { "local_Noise", "LuminanceSmoothing" },
	moire = { "local_Moire" },
}

local AI_MASK_TOOL_CANDIDATES = {
	subject = { "subject", "selectSubject", "person" },
	sky = { "sky", "selectSky" },
	people = { "people", "person" },
	person = { "person", "people" },
	object = { "object", "objects" },
	objects = { "objects", "object" },
	background = { "background", "subject", "selectSubject" },
}

local HSL_LABELS = {
	red = "Red",
	orange = "Orange",
	yellow = "Yellow",
	green = "Green",
	aqua = "Aqua",
	blue = "Blue",
	purple = "Purple",
	magenta = "Magenta",
}

-- Global recipe values are ABSOLUTE Lightroom slider values, never deltas.
--
-- Both producers agree on this: the LLM schema declares Lightroom's own absolute
-- ranges (`temperature` 2000..50000 Kelvin, `sharpening` 0..150), and the style
-- engine blends `canonical_settings`, which are read straight off the user's real
-- develop settings. Decisive is what the backend *measures*: the RAW decode, not
-- the photo's current edited state. Adding a recipe value on top of an existing
-- edit therefore double-counts that edit.
--
-- This used to be an additive merge for most keys, which was catastrophic for
-- the white balance (as-shot 5500 K + a recommended 5600 K produced 11100 K) and silently
-- wrong wherever Lightroom's default is non-zero: `ColorNoiseReduction` (25),
-- `SharpenDetail` (25), `GrainSize`, the vignette midpoints, and the wrapping
-- `SplitToning*Hue` angles.
--
-- Note the contrast with mask adjustments: `MASK_ADJUSTMENT_RANGES` in
-- `edit_recipe.rs` gives local temperature/tint as -100..100, because Lightroom's
-- *local* white balance genuinely is relative. That path is handled separately in
-- `applyMaskEdits` and is unaffected by this.

local DEVELOP_VALUE_BOUNDS = {
	Exposure2012 = { min = -5, max = 5 },
	Contrast2012 = { min = -100, max = 100 },
	Highlights2012 = { min = -100, max = 100 },
	Shadows2012 = { min = -100, max = 100 },
	Whites2012 = { min = -100, max = 100 },
	Blacks2012 = { min = -100, max = 100 },
	-- White balance lives in two families of keys, never in `Temp`, which is
	-- not what `getDevelopSettings` reports (none of the saved training
	-- examples carries it). A raw file carries Kelvin in
	-- `Temperature` and an absolute `Tint`; a rendered file (JPEG/TIFF/PNG, and
	-- a DNG converted from one) carries offsets from its own rendering in
	-- `IncrementalTemperature`/`IncrementalTint`. Each key has one scale, so the
	-- bounds no longer depend on the file.
	Temperature = { min = 2000, max = 50000 },
	Tint = { min = -150, max = 150 },
	IncrementalTemperature = { min = -100, max = 100 },
	IncrementalTint = { min = -100, max = 100 },
	Texture = { min = -100, max = 100 },
	Clarity2012 = { min = -100, max = 100 },
	Dehaze = { min = -100, max = 100 },
	Vibrance = { min = -100, max = 100 },
	Saturation = { min = -100, max = 100 },
	Sharpness = { min = 0, max = 150 },
	SharpenRadius = { min = 0.5, max = 3.0 },
	SharpenDetail = { min = 0, max = 100 },
	SharpenEdgeMasking = { min = 0, max = 100 },
	LuminanceSmoothing = { min = 0, max = 100 },
	LuminanceNoiseReductionDetail = { min = 0, max = 100 },
	LuminanceNoiseReductionContrast = { min = 0, max = 100 },
	ColorNoiseReduction = { min = 0, max = 100 },
	ColorNoiseReductionDetail = { min = 0, max = 100 },
	ColorNoiseReductionSmoothness = { min = 0, max = 100 },
	PostCropVignetteAmount = { min = -100, max = 100 },
	PostCropVignetteMidpoint = { min = 0, max = 100 },
	PostCropVignetteRoundness = { min = -100, max = 100 },
	PostCropVignetteFeather = { min = 0, max = 100 },
	PostCropVignetteHighlightContrast = { min = 0, max = 100 },
	GrainAmount = { min = 0, max = 100 },
	GrainSize = { min = 0, max = 100 },
	GrainFrequency = { min = 0, max = 100 },
	SplitToningShadowHue = { min = 0, max = 360, wrap = true },
	SplitToningShadowSaturation = { min = 0, max = 100 },
	SplitToningHighlightHue = { min = 0, max = 360, wrap = true },
	SplitToningHighlightSaturation = { min = 0, max = 100 },
	SplitToningBalance = { min = -100, max = 100 },
	ParametricHighlights = { min = -100, max = 100 },
	ParametricLights = { min = -100, max = 100 },
	ParametricDarks = { min = -100, max = 100 },
	ParametricShadows = { min = -100, max = 100 },
	ParametricShadowSplit = { min = 0, max = 100 },
	ParametricMidtoneSplit = { min = 0, max = 100 },
	ParametricHighlightSplit = { min = 0, max = 100 },
	CropLeft = { min = 0, max = 1 },
	CropRight = { min = 0, max = 1 },
	CropTop = { min = 0, max = 1 },
	CropBottom = { min = 0, max = 1 },
	CropAngle = { min = -45, max = 45 },
}

-- A value this large on the relative scale can only be Kelvin that reached us
-- anyway: an older backend, or a model that ignored the schema. Clamping it
-- would apply maximum warmth to every affected photo, so it is dropped
-- instead. Mirrors `KELVIN_LOOKING` in `edit_recipe.rs`. The same threshold
-- read the other way catches a relative value aimed at a raw file, which the
-- Kelvin clamp would otherwise turn into 2000 K, maximum cool.
local KELVIN_LOOKING = 500

-- The develop keys that carry a white balance, per family. See the note in
-- DEVELOP_VALUE_BOUNDS.
local WHITE_BALANCE_KEYS = {
	raw = { temperature = "Temperature", tint = "Tint" },
	non_raw = { temperature = "IncrementalTemperature", tint = "IncrementalTint" },
}

for _, label in pairs(HSL_LABELS) do
	DEVELOP_VALUE_BOUNDS["HueAdjustment" .. label] = { min = -100, max = 100 }
	DEVELOP_VALUE_BOUNDS["SaturationAdjustment" .. label] = { min = -100, max = 100 }
	DEVELOP_VALUE_BOUNDS["LuminanceAdjustment" .. label] = { min = -100, max = 100 }
end

local function appendWarning(warnings, text)
	if warnings and text and text ~= "" then
		table.insert(warnings, text)
	end
end

local function sortedKeys(tbl)
	local keys = {}
	for key in pairs(tbl or {}) do
		table.insert(keys, key)
	end
	table.sort(keys)
	return keys
end

local function tableCount(tbl)
	local count = 0
	for _ in pairs(tbl or {}) do
		count = count + 1
	end
	return count
end

local function getRecipeFromResponse(response)
	if type(response) ~= "table" then
		return nil
	end
	if type(response.edit) == "table" then
		return response.edit
	end
	if type(response.recipe) == "table" then
		return response.recipe
	end
	if type(response.global) == "table" or type(response.masks) == "table" then
		return response
	end
	return nil
end

local function buildHslDevelopSettings(hsl)
	local settings = {}
	if type(hsl) ~= "table" then
		return settings
	end

	for channel, adjustments in pairs(hsl) do
		local label = HSL_LABELS[channel]
		if label and type(adjustments) == "table" then
			if adjustments.hue ~= nil then
				settings["HueAdjustment" .. label] = adjustments.hue
			end
			if adjustments.saturation ~= nil then
				settings["SaturationAdjustment" .. label] = adjustments.saturation
			end
			if adjustments.luminance ~= nil then
				settings["LuminanceAdjustment" .. label] = adjustments.luminance
			end
		end
	end
	return settings
end

local function buildColorGradingDevelopSettings(colorGrading, warnings)
	local settings = {}
	if type(colorGrading) ~= "table" then
		return settings
	end

	local shadows = colorGrading.shadows
	if type(shadows) == "table" then
		if shadows.hue ~= nil then
			settings.SplitToningShadowHue = shadows.hue
		end
		if shadows.saturation ~= nil then
			settings.SplitToningShadowSaturation = shadows.saturation
		end
		if shadows.luminance ~= nil then
			appendWarning(
				warnings,
				"Shadow color grading luminance is not supported by Lightroom develop settings and was ignored."
			)
		end
	end

	local highlights = colorGrading.highlights
	if type(highlights) == "table" then
		if highlights.hue ~= nil then
			settings.SplitToningHighlightHue = highlights.hue
		end
		if highlights.saturation ~= nil then
			settings.SplitToningHighlightSaturation = highlights.saturation
		end
		if highlights.luminance ~= nil then
			appendWarning(
				warnings,
				"Highlight color grading luminance is not supported by Lightroom develop settings and was ignored."
			)
		end
	end

	if colorGrading.balance ~= nil then
		settings.SplitToningBalance = colorGrading.balance
	end
	if type(colorGrading.midtones) == "table" then
		appendWarning(
			warnings,
			"Midtone color grading is not currently mapped by the Lightroom plugin and was ignored."
		)
	end
	if type(colorGrading.global) == "table" then
		appendWarning(warnings, "Global color grading is not currently mapped by the Lightroom plugin and was ignored.")
	end
	if colorGrading.blending ~= nil then
		appendWarning(
			warnings,
			"Color grading blending is not currently mapped by the Lightroom plugin and was ignored."
		)
	end

	if next(settings) ~= nil then
		settings.EnableSplitToning = true
	end
	return settings
end

local function buildToneCurveSettings(toneCurve)
	local settings = {}
	if type(toneCurve) ~= "table" then
		return settings
	end

	if toneCurve.highlights ~= nil then
		settings.ParametricHighlights = toneCurve.highlights
	end
	if toneCurve.lights ~= nil then
		settings.ParametricLights = toneCurve.lights
	end
	if toneCurve.darks ~= nil then
		settings.ParametricDarks = toneCurve.darks
	end
	if toneCurve.shadows ~= nil then
		settings.ParametricShadows = toneCurve.shadows
	end
	if toneCurve.shadow_split ~= nil then
		settings.ParametricShadowSplit = toneCurve.shadow_split
	end
	if toneCurve.midtone_split ~= nil then
		settings.ParametricMidtoneSplit = toneCurve.midtone_split
	end
	if toneCurve.highlight_split ~= nil then
		settings.ParametricHighlightSplit = toneCurve.highlight_split
	end

	local pointCurve = toneCurve.point_curve
	local extendedPointCurve = toneCurve.extended_point_curve
	if type(pointCurve) == "table" then
		if type(pointCurve.master) == "table" and #pointCurve.master >= 4 then
			settings.ToneCurvePV2012 = pointCurve.master
		end
		if type(pointCurve.red) == "table" and #pointCurve.red >= 4 then
			settings.ToneCurvePV2012Red = pointCurve.red
		end
		if type(pointCurve.green) == "table" and #pointCurve.green >= 4 then
			settings.ToneCurvePV2012Green = pointCurve.green
		end
		if type(pointCurve.blue) == "table" and #pointCurve.blue >= 4 then
			settings.ToneCurvePV2012Blue = pointCurve.blue
		end
	end

	if type(extendedPointCurve) == "table" then
		if type(extendedPointCurve.master) == "table" and #extendedPointCurve.master >= 4 then
			settings.ExtendedToneCurvePV2012 = extendedPointCurve.master
		end
		if type(extendedPointCurve.red) == "table" and #extendedPointCurve.red >= 4 then
			settings.ExtendedToneCurvePV2012Red = extendedPointCurve.red
		end
		if type(extendedPointCurve.green) == "table" and #extendedPointCurve.green >= 4 then
			settings.ExtendedToneCurvePV2012Green = extendedPointCurve.green
		end
		if type(extendedPointCurve.blue) == "table" and #extendedPointCurve.blue >= 4 then
			settings.ExtendedToneCurvePV2012Blue = extendedPointCurve.blue
		end
	end

	-- Lightroom versions differ in which curve keys they honor on apply.
	-- Provide both standard and extended PV2012 keys when possible.
	if settings.ToneCurvePV2012 and settings.ExtendedToneCurvePV2012 == nil then
		settings.ExtendedToneCurvePV2012 = settings.ToneCurvePV2012
	end
	if settings.ToneCurvePV2012Red and settings.ExtendedToneCurvePV2012Red == nil then
		settings.ExtendedToneCurvePV2012Red = settings.ToneCurvePV2012Red
	end
	if settings.ToneCurvePV2012Green and settings.ExtendedToneCurvePV2012Green == nil then
		settings.ExtendedToneCurvePV2012Green = settings.ToneCurvePV2012Green
	end
	if settings.ToneCurvePV2012Blue and settings.ExtendedToneCurvePV2012Blue == nil then
		settings.ExtendedToneCurvePV2012Blue = settings.ToneCurvePV2012Blue
	end

	if
		settings.ToneCurvePV2012
		or settings.ToneCurvePV2012Red
		or settings.ToneCurvePV2012Green
		or settings.ToneCurvePV2012Blue
		or settings.ExtendedToneCurvePV2012
		or settings.ExtendedToneCurvePV2012Red
		or settings.ExtendedToneCurvePV2012Green
		or settings.ExtendedToneCurvePV2012Blue
	then
		settings.EnableToneCurve = true
		settings.ToneCurveName2012 = "Custom"
		settings.ToneCurveName = "Custom"
	end
	return settings
end

local function buildLensCorrectionSettings(lensCorrections)
	local settings = {}
	if type(lensCorrections) ~= "table" then
		return settings
	end
	if lensCorrections.enable_profile_corrections ~= nil then
		settings.EnableLensCorrections = lensCorrections.enable_profile_corrections
	end
	if lensCorrections.remove_chromatic_aberration ~= nil then
		settings.AutoLateralCA = lensCorrections.remove_chromatic_aberration
	end
	return settings
end

local function buildCropSettings(crop, warnings)
	local settings = {}
	if type(crop) ~= "table" then
		return settings
	end

	local left = crop.left
	local right = crop.right
	local top = crop.top
	local bottom = crop.bottom
	local angle = crop.angle

	-- Compatibility with alternate crop payload shape frequently used by LLMs.
	-- If canonical edges are absent, map x/y/width/height into edge coordinates.
	if
		(left == nil and right == nil and top == nil and bottom == nil)
		and crop.x ~= nil
		and crop.y ~= nil
		and crop.width ~= nil
		and crop.height ~= nil
	then
		left = crop.x
		top = crop.y
		right = crop.x + crop.width
		bottom = crop.y + crop.height
	end
	if angle == nil and crop.rotation ~= nil then
		angle = crop.rotation
	end

	if left ~= nil and right ~= nil and left >= right then
		appendWarning(warnings, "Crop was ignored because left >= right.")
		return settings
	end
	if top ~= nil and bottom ~= nil and top >= bottom then
		appendWarning(warnings, "Crop was ignored because top >= bottom.")
		return settings
	end

	if left ~= nil then
		settings.CropLeft = left
	end
	if right ~= nil then
		settings.CropRight = right
	end
	if top ~= nil then
		settings.CropTop = top
	end
	if bottom ~= nil then
		settings.CropBottom = bottom
	end
	if angle ~= nil then
		settings.CropAngle = angle
	end
	if next(settings) ~= nil then
		settings.HasCrop = true
	end
	return settings
end

local function mergeSettings(target, source)
	for key, value in pairs(source or {}) do
		target[key] = value
	end
end

local function normalizeDevelopValue(key, value)
	if type(value) ~= "number" then
		return value
	end
	local bounds = DEVELOP_VALUE_BOUNDS[key]
	if not bounds then
		return value
	end
	if bounds.wrap then
		local span = bounds.max - bounds.min
		if span <= 0 then
			return value
		end
		local shifted = value - bounds.min
		local wrapped = shifted - math.floor(shifted / span) * span
		return bounds.min + wrapped
	end
	if value < bounds.min then
		return bounds.min
	end
	if value > bounds.max then
		return bounds.max
	end
	return value
end

local function mergeGlobalDevelopSettings(currentSettings, aiSettings)
	local merged = {}
	-- Start with existing settings to preserve all state, including linked keys
	-- like crop coordinates and tone curves that aren't being touched by the AI.
	if type(currentSettings) == "table" then
		for k, v in pairs(currentSettings) do
			merged[k] = v
		end
	end

	-- Recipe values are absolute (see the note next to DEVELOP_VALUE_BOUNDS), so a
	-- key the recipe carries replaces the current value rather than adding to it.
	-- Keys the recipe does not mention keep whatever the photo already had.
	for key, value in pairs(aiSettings or {}) do
		merged[key] = normalizeDevelopValue(key, value)
	end
	return merged
end

---
-- Which family of white-balance keys a photo's develop settings use.
--
-- Lightroom answers this itself: a raw file's settings carry `Temperature`
-- (Kelvin), a rendered file's carry `IncrementalTemperature` (an offset from
-- its own rendering). The file format is only a fallback, because it is wrong
-- for a DNG converted from a JPEG: Lightroom counts that as a DNG, yet its
-- white balance is the rendered kind.
--
-- @param settings table|nil The photo's `getDevelopSettings()`, or nil when
--        they could not be read.
-- @param isRawFallback boolean|nil `Util.isRawPhoto(photo)`, used only when the
--        settings carry neither key.
-- @return string|nil "raw", "non_raw", or nil when neither source knows.
--
function DevelopEditManager.whiteBalanceFamily(settings, isRawFallback)
	if type(settings) == "table" then
		if settings.Temperature ~= nil then
			return "raw"
		end
		if settings.IncrementalTemperature ~= nil then
			return "non_raw"
		end
	end
	if isRawFallback == true then
		return "raw"
	end
	if isRawFallback == false then
		return "non_raw"
	end
	return nil
end

---
-- The white-balance family of a photo in the catalog. Needs an async task,
-- because it reads the develop settings. Sent to the backend as `is_raw`, so
-- the style engine picks examples of the same family that this plug-in will
-- later write the result into.
--
-- @param photo LrPhoto
-- @return string|nil "raw", "non_raw", or nil when unknown.
--
function DevelopEditManager.photoWhiteBalanceFamily(photo)
	if photo == nil then
		return nil
	end
	local ok, settingsOrErr = LrTasks.pcall(function()
		return photo:getDevelopSettings()
	end)
	local settings = nil
	if ok and type(settingsOrErr) == "table" then
		settings = settingsOrErr
	else
		log:warn(
			"DevelopEditManager.photoWhiteBalanceFamily: develop settings unavailable: " .. tostring(settingsOrErr)
		)
	end
	return DevelopEditManager.whiteBalanceFamily(settings, Util.isRawPhoto(photo))
end

local function familyLabel(family)
	if family == "raw" then
		return "raw files"
	end
	return "non-raw files (JPEG, TIFF, PNG, or a DNG converted from one)"
end

-- One family's temperature/tint pair as develop settings, with
-- `WhiteBalance = "Custom"` so Lightroom shows the numbers it was given rather
-- than a preset label that no longer describes them. Returns an empty table
-- (and says why) when the numbers are on the other family's scale.
local function mapWhiteBalanceValues(family, temperature, tint, warnings)
	local keys = WHITE_BALANCE_KEYS[family]
	local settings = {}
	if not keys then
		return settings
	end
	if type(temperature) == "number" then
		local looksKelvin = math.abs(temperature) > KELVIN_LOOKING
		if family == "non_raw" and looksKelvin then
			-- Clamping 6200 into -100..100 gives +100: a hard orange cast.
			-- Leaving the photo's own white balance alone is the lesser harm,
			-- and the tint that came with it is on the raw scale too.
			appendWarning(
				warnings,
				"White balance was not applied: the suggestion is a Kelvin value, but this photo is not a raw file, "
					.. "where Lightroom expects a relative -100 to +100 value instead."
			)
			return {}
		end
		if family == "raw" and not looksKelvin then
			appendWarning(
				warnings,
				"White balance was not applied: the suggestion is a relative value, but this photo is a raw file, "
					.. "where Lightroom expects a temperature in Kelvin."
			)
			return {}
		end
		settings[keys.temperature] = temperature
	end
	if type(tint) == "number" then
		settings[keys.tint] = tint
	end
	if next(settings) ~= nil then
		settings.WhiteBalance = "Custom"
	end
	return settings
end

local function warnUnknownFamily(warnings)
	appendWarning(
		warnings,
		"White balance was not applied: Lightroom did not report whether this photo is a raw file, "
			.. "so it is unclear which white-balance scale it uses."
	)
end

-- Develop settings for the recipe's white balance, written into the keys of
-- the photo's own family (`targetFamily`, from whiteBalanceFamily).
--
-- Two wire forms reach this:
--  * `recipe.white_balance = { mode, family, temperature?, tint? }`, next to
--    `global`/`masks`: what the style engine sends. `family` says which
--    scale the numbers are on; a mismatch with the photo drops them, since a
--    Kelvin value has no meaning as an offset and vice versa.
--  * `recipe.global.temperature`/`tint` (or the LLM's `global.white_balance`
--    alias object): an older backend, or the LLM fallback. These carry no
--    family, so they are read on the photo's own scale.
--
-- Never writes `Temp`: it is not a key `getDevelopSettings` reports, and its
-- scale changes with the file, which is how a Kelvin value used to land on a
-- JPEG. (TaskDevelopExperiments writes it on purpose, to find out what
-- Lightroom makes of it.)
local function buildWhiteBalanceSettings(recipe, targetFamily, warnings)
	local wb = type(recipe) == "table" and recipe.white_balance or nil
	if type(wb) == "table" then
		local mode = wb.mode
		if mode ~= nil and mode ~= "Custom" then
			-- A mode without numbers ("Auto", "Daylight", ...) is meant to be
			-- re-evaluated by Lightroom for this frame. Whether
			-- applyDevelopSettings does that is not verified yet (experiment
			-- E1), and the backend does not send such modes until it is; if
			-- one arrives anyway it is reported rather than applied blind.
			appendWarning(
				warnings,
				"White balance '"
					.. tostring(mode)
					.. "' was not applied: this plug-in version only transfers a custom white balance. "
					.. "Update the plug-in to transfer this white-balance mode."
			)
			return {}
		end
		if targetFamily == nil then
			warnUnknownFamily(warnings)
			return {}
		end
		local family = wb.family
		if not WHITE_BALANCE_KEYS[family] then
			appendWarning(
				warnings,
				"White balance was not applied: the backend sent an unrecognised white-balance scale ("
					.. tostring(family)
					.. "). Update the plug-in and the backend together."
			)
			return {}
		end
		if family ~= targetFamily then
			appendWarning(
				warnings,
				"White balance was not applied: it was learned from "
					.. familyLabel(family)
					.. ", and this photo is one of the "
					.. familyLabel(targetFamily)
					.. "."
			)
			return {}
		end
		return mapWhiteBalanceValues(targetFamily, wb.temperature, wb.tint, warnings)
	end

	local globalSettings = type(recipe) == "table" and type(recipe.global) == "table" and recipe.global or {}
	local temperature = globalSettings.temperature
	local tint = globalSettings.tint
	if temperature == nil and tint == nil and type(globalSettings.white_balance) == "table" then
		temperature = globalSettings.white_balance.temperature
		tint = globalSettings.white_balance.tint
	end
	if type(temperature) ~= "number" and type(tint) ~= "number" then
		return {}
	end
	if targetFamily == nil then
		warnUnknownFamily(warnings)
		return {}
	end
	return mapWhiteBalanceValues(targetFamily, temperature, tint, warnings)
end

local function roundNumber(value)
	if value >= 0 then
		return math.floor(value + 0.5)
	end
	return -math.floor(-value + 0.5)
end

local function signedNumber(value)
	local rounded = roundNumber(value)
	if rounded > 0 then
		return "+" .. tostring(rounded)
	end
	return tostring(rounded)
end

---
-- A white balance as one readable phrase, for the review dialog.
--
-- @param wb table `{ mode?, family?, temperature?, tint? }`.
-- @return string|nil e.g. "Custom 5600 K, tint +5"; nil for a non-table.
--
function DevelopEditManager.formatWhiteBalance(wb)
	if type(wb) ~= "table" then
		return nil
	end
	local parts = {}
	local head = wb.mode ~= nil and tostring(wb.mode) or nil
	if type(wb.temperature) == "number" then
		local temperature
		if wb.family == "raw" then
			temperature = tostring(roundNumber(wb.temperature)) .. " K"
		elseif wb.family == "non_raw" then
			temperature = "temperature " .. signedNumber(wb.temperature)
		else
			temperature = "temperature " .. tostring(roundNumber(wb.temperature))
		end
		if head and wb.family == "raw" then
			head = head .. " " .. temperature
		elseif head then
			table.insert(parts, temperature)
		else
			head = temperature
		end
	end
	if type(wb.tint) == "number" then
		table.insert(parts, "tint " .. signedNumber(wb.tint))
	end
	if head then
		table.insert(parts, 1, head)
	end
	if #parts == 0 then
		return "unchanged"
	end
	return table.concat(parts, ", ")
end

local function formatGlobalSettings(globalSettings)
	local lines = {}
	for _, key in ipairs(sortedKeys(globalSettings or {})) do
		if
			key ~= "hsl"
			and key ~= "color_grading"
			and key ~= "tone_curve"
			and key ~= "lens_corrections"
			and key ~= "crop"
			and key ~= "white_balance"
		then
			local value = globalSettings[key]
			if type(value) == "table" then
				-- A group this dialog has no dedicated line for (a newer
				-- backend may send one). Its address is no use to anyone.
				table.insert(lines, "- " .. tostring(key) .. ": " .. tostring(tableCount(value)) .. " setting(s)")
			else
				table.insert(lines, "- " .. tostring(key) .. ": " .. tostring(value))
			end
		end
	end
	-- The LLM's `{ temperature, tint }` alias; the style engine's white balance
	-- sits next to `global` and is shown by formatRecipeDetails.
	if type(globalSettings.white_balance) == "table" then
		table.insert(lines, "- white_balance: " .. DevelopEditManager.formatWhiteBalance(globalSettings.white_balance))
	end
	if type(globalSettings.hsl) == "table" then
		table.insert(lines, "- hsl: " .. tostring(tableCount(globalSettings.hsl)) .. " channel(s)")
	end
	if type(globalSettings.color_grading) == "table" then
		table.insert(lines, "- color_grading: enabled")
	end
	if type(globalSettings.tone_curve) == "table" then
		table.insert(lines, "- tone_curve: enabled")
	end
	if type(globalSettings.lens_corrections) == "table" then
		table.insert(lines, "- lens_corrections: enabled")
	end
	if type(globalSettings.crop) == "table" then
		table.insert(lines, "- crop: enabled")
	end
	return lines
end

function DevelopEditManager.formatRecipeDetails(response)
	local recipe = getRecipeFromResponse(response)
	if not recipe then
		return LOC("$$$/LrGeniusAI/DevelopEdit/NoRecipe=No edit recipe available.")
	end

	local lines = {}

	-- Style Engine Metadata
	if response and response.engine then
		local engineName = response.engine == "style" and "Photographer Style Engine" or "LLM Style Fallback"
		table.insert(lines, "Engine: " .. engineName)
		if response.confidence then
			local conf = math.floor(response.confidence * 100)
			table.insert(lines, "Match Confidence: " .. tostring(conf) .. "%")
		end
		if response.matched_examples then
			table.insert(lines, "Matched Examples: " .. tostring(response.matched_examples))
		end
		if response.matched_filenames and #response.matched_filenames > 0 then
			table.insert(lines, "Source Styles: " .. table.concat(response.matched_filenames, ", "))
		end
		table.insert(lines, "")
	end

	table.insert(lines, "Summary")
	table.insert(lines, recipe.summary or "AI-generated Lightroom edit recipe")
	table.insert(lines, "")

	-- What the photograph itself would not take, as opposed to what the model
	-- decided. Present only when a limit actually changed the recipe, so an
	-- edit that came back as generated shows nothing here. The sentences are
	-- written by the backend next to the thresholds that produce them
	-- (`GuardrailReason::explanation`); duplicating them in Lua would let the
	-- two drift apart.
	if type(response) == "table" and type(response.guardrail_explanations) == "table" then
		local explanations = response.guardrail_explanations
		if #explanations > 0 then
			table.insert(lines, LOC("$$$/LrGeniusAI/DevelopEdit/AdjustedForPhoto=Adjusted for this photo"))
			for _, explanation in ipairs(explanations) do
				table.insert(lines, "- " .. tostring(explanation))
			end
			table.insert(lines, "")
		end
	end

	local globalSettings = recipe.global or {}
	table.insert(lines, "Global adjustments")
	local globalLines = formatGlobalSettings(globalSettings)
	if type(recipe.white_balance) == "table" then
		table.insert(globalLines, 1, "- White balance: " .. DevelopEditManager.formatWhiteBalance(recipe.white_balance))
	end
	if #globalLines == 0 then
		table.insert(lines, "- none")
	else
		for _, line in ipairs(globalLines) do
			table.insert(lines, line)
		end
	end
	table.insert(lines, "")

	table.insert(lines, "Masks")
	local masks = recipe.masks or {}
	if #masks == 0 then
		table.insert(lines, "- none")
	else
		for _, mask in ipairs(masks) do
			local count = tableCount(mask.adjustments or {})
			table.insert(lines, "- " .. tostring(mask.kind or "mask") .. " (" .. tostring(count) .. " adjustment(s))")
		end
	end
	table.insert(lines, "")

	-- The recipe's own notes, then the response's (the style engine puts its
	-- white-balance and confidence notes there, not in the recipe), so the
	-- user sees why a setting was left out before deciding to apply.
	table.insert(lines, "Warnings")
	local warnings = {}
	local seen = {}
	local function addWarning(text)
		text = text ~= nil and tostring(text) or ""
		if text ~= "" and not seen[text] then
			seen[text] = true
			table.insert(warnings, text)
		end
	end
	for _, warning in ipairs(type(recipe.warnings) == "table" and recipe.warnings or {}) do
		addWarning(warning)
	end
	for _, warning in ipairs(Util.responseWarnings(response)) do
		addWarning(warning)
	end
	if #warnings == 0 then
		table.insert(lines, "- none")
	else
		for _, warning in ipairs(warnings) do
			table.insert(lines, "- " .. warning)
		end
	end

	return table.concat(lines, "\n")
end

function DevelopEditManager.persistEditRecipe(photo, response, warnings, status)
	log:trace("DevelopEditManager.persistEditRecipe: start status=" .. tostring(status))
	local okRecipe, recipeOrErr = LrTasks.pcall(function()
		return getRecipeFromResponse(response)
	end)
	if not okRecipe then
		log:error("DevelopEditManager.persistEditRecipe: getRecipeFromResponse failed: " .. tostring(recipeOrErr))
		return
	end
	local recipe = recipeOrErr
	if not photo or not recipe then
		log:error("DevelopEditManager.persistEditRecipe: missing photo or recipe")
		return
	end

	log:trace("DevelopEditManager.persistEditRecipe: recipe resolved, building warnings")
	local allWarnings = {}
	if type(recipe.warnings) == "table" then
		for _, warning in ipairs(recipe.warnings) do
			table.insert(allWarnings, tostring(warning))
		end
	end
	if type(warnings) == "table" then
		for _, warning in ipairs(warnings) do
			table.insert(allWarnings, tostring(warning))
		end
	end

	log:trace("DevelopEditManager.persistEditRecipe: encoding recipe JSON")
	local okEncode, recipeJsonOrErr = LrTasks.pcall(function()
		return JSON:encode(recipe)
	end)
	if not okEncode then
		log:error("DevelopEditManager.persistEditRecipe: JSON encode failed: " .. tostring(recipeJsonOrErr))
		recipeJsonOrErr = "{}"
	end
	local recipeJson = recipeJsonOrErr

	local warningText = #allWarnings > 0 and table.concat(allWarnings, "\n") or ""
	if #warningText > 500 then
		warningText = string.sub(warningText, 1, 500)
		appendWarning(allWarnings, "Warnings were truncated for Lightroom metadata field size limits.")
	end
	log:trace("DevelopEditManager.persistEditRecipe: warningText length=" .. tostring(#warningText))
	local runDate = (type(response) == "table" and (response.edit_rundate or response.ai_rundate)) or ""
	if runDate == "" then
		runDate = LrDate.timeToW3CDate(LrDate.currentTime())
	end
	local modelName = ""
	if type(response) == "table" then
		modelName = response.edit_model or response.ai_model or ""
	end

	log:trace("DevelopEditManager.persistEditRecipe: entering catalog write")
	local catalog = LrApplication.activeCatalog()
	local okWrite, writeErr = LrTasks.pcall(function()
		-- withPrivateWriteAccessDo signature here is (callback [, options]).
		-- Passing an action-name string first can trigger obscure runtime errors in LR.
		catalog:withPrivateWriteAccessDo(function()
			photo:setPropertyForPlugin(_PLUGIN, "aiEditLastRun", tostring(runDate))
			photo:setPropertyForPlugin(_PLUGIN, "aiEditModel", tostring(modelName))
			photo:setPropertyForPlugin(_PLUGIN, "aiEditSummary", tostring(recipe.summary or ""))
			photo:setPropertyForPlugin(_PLUGIN, "aiEditWarnings", warningText)
			photo:setPropertyForPlugin(_PLUGIN, "aiEditRecipe", tostring(recipeJson or ""))
			photo:setPropertyForPlugin(_PLUGIN, "aiEditStatus", tostring(status or "generated"))
		end, Defaults.catalogWriteAccessOptions)
	end)
	if not okWrite then
		log:error("DevelopEditManager.persistEditRecipe: catalog write failed: " .. tostring(writeErr))
		return
	end
	log:trace("DevelopEditManager.persistEditRecipe: done warningsCount=" .. tostring(#allWarnings))
end

-- `whiteBalanceFamily` is the photo's own family ("raw", "non_raw" or nil),
-- from DevelopEditManager.whiteBalanceFamily.
local function buildDevelopSettings(recipe, warnings, whiteBalanceFamily)
	local developSettings = {}
	local globalSettings = recipe.global or {}

	for key, lrKey in pairs(GLOBAL_KEY_MAP) do
		local value = globalSettings[key]
		if value ~= nil then
			developSettings[lrKey] = value
		end
	end

	-- A white balance that does not fit the photo is left out here rather than
	-- removed after the merge: the merge starts from the photo's current
	-- settings, so never adding a key leaves the photo's own white balance
	-- alone, whereas removing it later would take that with it.
	mergeSettings(developSettings, buildWhiteBalanceSettings(recipe, whiteBalanceFamily, warnings))

	-- Respect the RAW profile (Adobe Color, Camera Neutral, ...) so the baseline the
	-- recipe was solved against is the one Lightroom actually renders.
	--
	-- This wrote `CameraConfig` for a long time, which is not a Lightroom develop
	-- key at all — the SDK calls it `CameraProfile`. The branch was doubly dead
	-- because `profile` is not in the recipe schema either, so nothing reached it.
	-- The key is corrected here so the branch works once the schema carries it.
	if globalSettings.profile then
		developSettings["CameraProfile"] = globalSettings.profile
	end

	mergeSettings(developSettings, buildHslDevelopSettings(globalSettings.hsl))
	mergeSettings(developSettings, buildColorGradingDevelopSettings(globalSettings.color_grading, warnings))
	mergeSettings(developSettings, buildToneCurveSettings(globalSettings.tone_curve))
	mergeSettings(developSettings, buildLensCorrectionSettings(globalSettings.lens_corrections))
	mergeSettings(developSettings, buildCropSettings(globalSettings.crop, warnings))

	return developSettings
end

-- Internal seams exposed for the headless unit tests in `plugin/spec`. These are
-- the functions that turn a recipe into Lightroom develop settings, which is
-- where value-semantics bugs surface; they are not part of the module's contract
-- for callers running inside Lightroom.
DevelopEditManager.internal = {
	normalizeDevelopValue = normalizeDevelopValue,
	mergeGlobalDevelopSettings = mergeGlobalDevelopSettings,
	buildDevelopSettings = buildDevelopSettings,
	buildWhiteBalanceSettings = buildWhiteBalanceSettings,
}

local function focusPhotoInDevelop(photo, warnings)
	local catalog = LrApplication.activeCatalog()
	local ok, err = LrTasks.pcall(function()
		catalog:setSelectedPhotos(photo, { photo })
		LrApplicationView.switchToModule("develop")
		LrTasks.sleep(0.2)
	end)
	if not ok then
		-- Fallback for cases where the current source doesn't contain the photo.
		local fallbackOk, fallbackErr = LrTasks.pcall(function()
			catalog:setActiveSources({ catalog.kAllPhotos })
			LrTasks.sleep(0.2)
			catalog:setSelectedPhotos(photo, { photo })
			LrApplicationView.switchToModule("develop")
			LrTasks.sleep(0.2)
		end)
		if fallbackOk then
			appendWarning(
				warnings,
				"Photo was not available in the current source; temporarily switched to All Photos for mask application."
			)
			return true
		end
		err = fallbackErr
	end
	if not ok then
		appendWarning(
			warnings,
			"Could not switch Lightroom to the Develop module for mask application: " .. tostring(err)
		)
		return false
	end
	return true
end

local function applyGlobalDevelopSettings(photo, recipe, warnings)
	log:trace("DevelopEditManager.applyGlobalDevelopSettings: start")
	local okCurrent, currentOrErr = LrTasks.pcall(function()
		return photo:getDevelopSettings()
	end)
	local currentSettings = nil
	if okCurrent and type(currentOrErr) == "table" then
		currentSettings = currentOrErr
	end
	-- Read from the photo rather than passed in: the backend was told the
	-- same thing when the recipe was generated, but a recipe can also be
	-- re-applied later, and the photo is the authority either way.
	local wbFamily = DevelopEditManager.whiteBalanceFamily(currentSettings, Util.isRawPhoto(photo))
	local developSettings = buildDevelopSettings(recipe, warnings, wbFamily)
	local cropInRecipe = recipe and recipe.global and recipe.global.crop
	if type(cropInRecipe) == "table" then
		log:trace(
			"DevelopEditManager.applyGlobalDevelopSettings: crop recipe left="
				.. tostring(cropInRecipe.left)
				.. " right="
				.. tostring(cropInRecipe.right)
				.. " top="
				.. tostring(cropInRecipe.top)
				.. " bottom="
				.. tostring(cropInRecipe.bottom)
				.. " x="
				.. tostring(cropInRecipe.x)
				.. " y="
				.. tostring(cropInRecipe.y)
				.. " width="
				.. tostring(cropInRecipe.width)
				.. " height="
				.. tostring(cropInRecipe.height)
				.. " angle="
				.. tostring(cropInRecipe.angle)
				.. " rotation="
				.. tostring(cropInRecipe.rotation)
		)
	else
		log:trace("DevelopEditManager.applyGlobalDevelopSettings: no crop in recipe")
	end
	if next(developSettings) == nil then
		log:trace("DevelopEditManager.applyGlobalDevelopSettings: nothing to apply")
		return true
	end

	local mergedSettings
	if currentSettings then
		mergedSettings = mergeGlobalDevelopSettings(currentSettings, developSettings)
	else
		-- Merged onto nothing rather than used as-is, so the values are still
		-- clamped to Lightroom's slider ranges.
		mergedSettings = mergeGlobalDevelopSettings({}, developSettings)
		appendWarning(
			warnings,
			"Could not read current develop settings for additive merge; AI edits were applied directly."
		)
		log:warn(
			"DevelopEditManager.applyGlobalDevelopSettings current settings unavailable: " .. tostring(currentOrErr)
		)
	end

	local catalog = LrApplication.activeCatalog()

	if type(cropInRecipe) == "table" or developSettings.HasCrop then
		log:trace("DevelopEditManager.applyGlobalDevelopSettings: crop detected, ensuring Develop module for refresh")
		focusPhotoInDevelop(photo, warnings)
	end

	local ok, err = LrTasks.pcall(function()
		catalog:withWriteAccessDo("Apply AI Lightroom develop settings", function()
			photo:applyDevelopSettings(mergedSettings)
		end, Defaults.catalogWriteAccessOptions)
	end)
	if not ok then
		appendWarning(warnings, "Failed to apply global develop settings: " .. tostring(err))
		log:error("DevelopEditManager.applyGlobalDevelopSettings failed: " .. tostring(err))
		return false
	end
	local okReadBack, afterOrErr = LrTasks.pcall(function()
		return photo:getDevelopSettings()
	end)
	if okReadBack and type(afterOrErr) == "table" then
		log:trace(
			"DevelopEditManager.applyGlobalDevelopSettings crop readback HasCrop="
				.. tostring(afterOrErr.HasCrop)
				.. " CropLeft="
				.. tostring(afterOrErr.CropLeft)
				.. " CropRight="
				.. tostring(afterOrErr.CropRight)
				.. " CropTop="
				.. tostring(afterOrErr.CropTop)
				.. " CropBottom="
				.. tostring(afterOrErr.CropBottom)
				.. " CropAngle="
				.. tostring(afterOrErr.CropAngle)
		)
	else
		log:trace("DevelopEditManager.applyGlobalDevelopSettings crop readback unavailable: " .. tostring(afterOrErr))
	end
	log:trace("DevelopEditManager.applyGlobalDevelopSettings: success")
	return true
end

local function supportsMaskAutomation()
	return type(LrDevelopController) == "table" and type(LrDevelopController.createNewMask) == "function"
end

local function applyMaskEdits(photo, recipe, warnings)
	log:trace("DevelopEditManager.applyMaskEdits: start")
	local masks = recipe.masks or {}
	if #masks == 0 then
		log:trace("DevelopEditManager.applyMaskEdits: no masks")
		return true
	end

	if not supportsMaskAutomation() then
		appendWarning(
			warnings,
			"Lightroom mask automation is unavailable in this Lightroom SDK version. Mask edits were stored but not applied."
		)
		log:warn("DevelopEditManager.applyMaskEdits: mask automation unavailable")
		return false
	end

	if not focusPhotoInDevelop(photo, warnings) then
		return false
	end
	if type(LrDevelopController.goToMasking) == "function" then
		LrTasks.pcall(function()
			LrDevelopController.goToMasking()
		end)
	end

	local function findMaskGroup(settings)
		if type(settings) ~= "table" then
			return nil, nil
		end
		if type(settings.MaskGroup) == "table" then
			return "MaskGroup", settings.MaskGroup
		end
		if type(settings.MaskGroupBasedCorrections) == "table" then
			return "MaskGroupBasedCorrections", settings.MaskGroupBasedCorrections
		end
		return nil, nil
	end

	local function logMaskedDevelopSnapshot(stageLabel)
		local ok, settingsOrErr = LrTasks.pcall(function()
			return photo:getDevelopSettings()
		end)
		if not ok or type(settingsOrErr) ~= "table" then
			log:trace("DevelopEditManager.applyMaskEdits snapshot(" .. tostring(stageLabel) .. "): no develop settings")
			return
		end
		local groupKey, groupMasks = findMaskGroup(settingsOrErr)
		if type(groupMasks) ~= "table" then
			local maskLikeKeys = {}
			for key, value in pairs(settingsOrErr) do
				if type(key) == "string" and string.find(key, "Mask") and type(value) == "table" then
					table.insert(maskLikeKeys, key)
				end
			end
			table.sort(maskLikeKeys)
			log:trace(
				"DevelopEditManager.applyMaskEdits snapshot("
					.. tostring(stageLabel)
					.. "): no mask group found; mask-like keys="
					.. tostring(table.concat(maskLikeKeys, ","))
			)
			return
		end
		local dump = ""
		local okDump, dumpOrErr = LrTasks.pcall(function()
			return Util.dumpTable(masks)
		end)
		if okDump and type(dumpOrErr) == "string" then
			dump = dumpOrErr
		end
		if #dump > 1800 then
			dump = string.sub(dump, 1, 1800) .. "...(truncated)"
		end
		log:trace(
			"DevelopEditManager.applyMaskEdits snapshot("
				.. tostring(stageLabel)
				.. "): group="
				.. tostring(groupKey)
				.. " maskCount="
				.. tostring(#groupMasks)
				.. " masks="
				.. tostring(dump)
		)
	end

	local function applyMaskAdjustmentsViaDevelopSettings(maskKind, adjustments)
		if type(adjustments) ~= "table" then
			return false, "no adjustments"
		end
		local catalog = LrApplication.activeCatalog()
		local ok, err = LrTasks.pcall(function()
			catalog:withWriteAccessDo("Apply AI mask adjustments via develop settings", function()
				local settings = photo:getDevelopSettings()
				local _, maskGroup = findMaskGroup(settings)
				if type(maskGroup) ~= "table" or #maskGroup == 0 then
					error("mask group not available in develop settings")
				end

				-- Newly created mask is typically appended; target the latest one.
				local targetMask = maskGroup[#maskGroup]
				if type(targetMask) ~= "table" then
					error("last mask entry is not a table")
				end
				local correction = targetMask.Correction
				if type(correction) ~= "table" then
					correction = targetMask.correction
				end
				if type(correction) ~= "table" then
					correction = targetMask.Adjustments
				end
				if type(correction) ~= "table" then
					correction = targetMask.adjustments
				end
				if type(correction) ~= "table" then
					correction = {}
					targetMask.Correction = correction
				end

				for key, value in pairs(adjustments) do
					local candidates = MASK_KEY_CANDIDATES[key]
					local written = false
					if candidates and #candidates > 0 then
						for _, candidate in ipairs(candidates) do
							correction[candidate] = value
							written = true
						end
					end
					if not written then
						appendWarning(
							warnings,
							"Mask adjustment '" .. tostring(key) .. "' is not currently supported and was ignored."
						)
					end
				end

				photo:applyDevelopSettings(settings)
				if type(photo.updateAISettings) == "function" then
					photo:updateAISettings()
				end
			end, Defaults.catalogWriteAccessOptions)
		end)
		if not ok then
			return false, err
		end
		log:trace(
			"DevelopEditManager.applyMaskEdits applied adjustments via develop settings for mask kind="
				.. tostring(maskKind)
		)
		return true, nil
	end

	local function readMaskList()
		if type(LrDevelopController.getAllMasks) ~= "function" then
			return {}
		end
		local ok, masksOrErr = LrTasks.pcall(function()
			return LrDevelopController.getAllMasks()
		end)
		if not ok or type(masksOrErr) ~= "table" then
			return {}
		end
		return masksOrErr
	end

	local function extractMaskId(maskItem)
		if type(maskItem) == "string" or type(maskItem) == "number" then
			return tostring(maskItem)
		end
		if type(maskItem) == "table" then
			if maskItem.id ~= nil then
				return tostring(maskItem.id)
			end
			if maskItem.maskId ~= nil then
				return tostring(maskItem.maskId)
			end
			if maskItem.uuid ~= nil then
				return tostring(maskItem.uuid)
			end
		end
		return nil
	end

	local function buildMaskIdSet(maskList)
		local ids = {}
		for _, item in ipairs(maskList or {}) do
			local id = extractMaskId(item)
			if id then
				ids[id] = true
			end
		end
		return ids
	end

	local function findNewMaskId(beforeMasks, afterMasks)
		local beforeIds = buildMaskIdSet(beforeMasks)
		for _, item in ipairs(afterMasks or {}) do
			local id = extractMaskId(item)
			if id and not beforeIds[id] then
				return id
			end
		end
		return nil
	end

	local function selectMaskById(maskId)
		if not maskId or type(LrDevelopController.selectMask) ~= "function" then
			return false
		end
		local ok = LrTasks.pcall(function()
			LrDevelopController.selectMask(maskId)
		end)
		return ok == true
	end

	local function getSelectedMaskId()
		if type(LrDevelopController.getSelectedMask) ~= "function" then
			return nil
		end
		local ok, selectedOrErr = LrTasks.pcall(function()
			return LrDevelopController.getSelectedMask()
		end)
		if not ok then
			return nil
		end
		local selectedDump = ""
		local okDump, dumpOrErr = LrTasks.pcall(function()
			return Util.dumpTable(selectedOrErr)
		end)
		if okDump and type(dumpOrErr) == "string" then
			selectedDump = dumpOrErr
			if #selectedDump > 800 then
				selectedDump = string.sub(selectedDump, 1, 800) .. "...(truncated)"
			end
		end
		log:trace("DevelopEditManager.applyMaskEdits selectedMask raw=" .. tostring(selectedDump))
		return extractMaskId(selectedOrErr)
	end

	local function getAiMaskToolCandidates(maskKind)
		local key = string.lower(tostring(maskKind or ""))
		local mapped = AI_MASK_TOOL_CANDIDATES[key]
		if mapped and #mapped > 0 then
			return mapped
		end
		return { key }
	end

	local function selectAiMaskTool(toolToken)
		if type(LrDevelopController.selectMaskTool) ~= "function" then
			return false, "selectMaskTool unavailable"
		end
		local okOneArg, errOneArg = LrTasks.pcall(function()
			LrDevelopController.selectMaskTool(toolToken)
		end)
		if okOneArg then
			return true, nil
		end
		local okTwoArgs, errTwoArgs = LrTasks.pcall(function()
			LrDevelopController.selectMaskTool("aiSelection", toolToken)
		end)
		if okTwoArgs then
			return true, nil
		end
		return false, errTwoArgs or errOneArg
	end

	local function createAiSelectionMask(toolToken)
		local okWithHint, idOrErrWithHint = LrTasks.pcall(function()
			return LrDevelopController.createNewMask("aiSelection", toolToken)
		end)
		if okWithHint then
			return true, extractMaskId(idOrErrWithHint), nil
		end
		local okNoHint, idOrErrNoHint = LrTasks.pcall(function()
			return LrDevelopController.createNewMask("aiSelection")
		end)
		if okNoHint then
			return true, extractMaskId(idOrErrNoHint), nil
		end
		return false, nil, idOrErrWithHint or idOrErrNoHint
	end

	local function createMaskForKind(maskKind)
		local toolCandidates = getAiMaskToolCandidates(maskKind)
		local lastAiErr = nil
		for _, toolToken in ipairs(toolCandidates) do
			local selectedTool, selectErr = selectAiMaskTool(toolToken)
			if not selectedTool then
				lastAiErr = selectErr
			end
			local created, createdMaskId, createErr = createAiSelectionMask(toolToken)
			if created then
				log:trace(
					"DevelopEditManager.applyMaskEdits create mask kind="
						.. tostring(maskKind)
						.. " using ai tool token="
						.. tostring(toolToken)
						.. " selectedTool="
						.. tostring(selectedTool)
						.. " createdMaskId="
						.. tostring(createdMaskId)
				)
				return true, createdMaskId, nil
			end
			lastAiErr = createErr or lastAiErr
			log:trace(
				"DevelopEditManager.applyMaskEdits ai create failed kind="
					.. tostring(maskKind)
					.. " token="
					.. tostring(toolToken)
					.. " err="
					.. tostring(createErr)
			)
		end

		local okBrush, errBrush = LrTasks.pcall(function()
			return LrDevelopController.createNewMask("brush")
		end)
		if okBrush then
			appendWarning(warnings, "Mask kind '" .. tostring(maskKind) .. "' fell back to brush; refine manually.")
			return true, extractMaskId(errBrush), nil
		end

		return false, nil, lastAiErr or errBrush
	end

	local function waitForMaskId(beforeMasks, immediateMaskId)
		if immediateMaskId then
			return immediateMaskId
		end
		for _ = 1, 12 do
			local masksAfter = readMaskList()
			local newMaskId = findNewMaskId(beforeMasks, masksAfter)
			if newMaskId then
				return newMaskId
			end
			local selectedMaskId = getSelectedMaskId()
			if selectedMaskId then
				return selectedMaskId
			end
			if #masksAfter > 0 then
				local lastId = extractMaskId(masksAfter[#masksAfter])
				if lastId then
					return lastId
				end
			end
			LrTasks.sleep(0.1)
		end
		return nil
	end

	for _, mask in ipairs(masks) do
		local maskKind = tostring(mask.kind or "")
		local ok, err = LrTasks.pcall(function()
			logMaskedDevelopSnapshot("before_" .. maskKind)
			local masksBefore = readMaskList()
			local created, createdMaskId, createErr = createMaskForKind(maskKind)
			if not created then
				error("createNewMask failed: " .. tostring(createErr))
			end
			local newMaskId = waitForMaskId(masksBefore, createdMaskId)
			local hasMaskContext
			if newMaskId then
				local selected = selectMaskById(newMaskId)
				hasMaskContext = selected or type(LrDevelopController.selectMask) ~= "function"
				log:trace(
					"DevelopEditManager.applyMaskEdits created mask kind="
						.. tostring(maskKind)
						.. " newMaskId="
						.. tostring(newMaskId)
						.. " selectOk="
						.. tostring(selected)
				)
			else
				hasMaskContext = false
				log:trace(
					"DevelopEditManager.applyMaskEdits created mask kind="
						.. tostring(maskKind)
						.. " but could not identify new mask id"
				)
			end
			logMaskedDevelopSnapshot("after_create_" .. maskKind)

			-- AI mask generation can complete asynchronously; give LR a moment.
			LrTasks.sleep(0.35)

			-- Best-effort to ensure local adjustment context is active.
			LrTasks.pcall(function()
				LrDevelopController.setValue("local_Amount", 100)
			end)
			local shouldInvert = mask.invert or (string.lower(maskKind) == "background")
			if shouldInvert and type(LrDevelopController.toggleInvertMaskTool) == "function" then
				LrDevelopController.toggleInvertMaskTool()
			end
			local controllerAppliedCount = 0
			if type(LrDevelopController.setValue) == "function" then
				for key, value in pairs(mask.adjustments or {}) do
					local candidates = MASK_KEY_CANDIDATES[key]
					if candidates and #candidates > 0 then
						local applied = false
						local lastErr = nil
						for _, candidate in ipairs(candidates) do
							local setOk, setErr = LrTasks.pcall(function()
								LrDevelopController.setValue(candidate, value)
							end)
							if setOk then
								local readBack = nil
								local readBackOk = false
								if type(LrDevelopController.getValue) == "function" then
									local rbOk, rbVal = LrTasks.pcall(function()
										return LrDevelopController.getValue(candidate)
									end)
									if rbOk then
										readBack = rbVal
										readBackOk = true
									end
								end
								-- Lightroom may apply local mask adjustments even when getValue() cannot
								-- read the local slider (returns nil on some SDK versions).
								applied = hasMaskContext
								if hasMaskContext then
									controllerAppliedCount = controllerAppliedCount + 1
								end
								if readBackOk then
									log:trace(
										"DevelopEditManager.applyMaskEdits applied "
											.. tostring(key)
											.. " via "
											.. tostring(candidate)
											.. "="
											.. tostring(value)
											.. " readBack="
											.. tostring(readBack)
									)
								else
									log:trace(
										"DevelopEditManager.applyMaskEdits applied "
											.. tostring(key)
											.. " via "
											.. tostring(candidate)
											.. "="
											.. tostring(value)
											.. " readBack=unavailable"
									)
								end
								if not hasMaskContext then
									log:trace(
										"DevelopEditManager.applyMaskEdits mask context missing while setting "
											.. tostring(candidate)
											.. "; treating as unverified"
									)
								end
								break
							else
								lastErr = setErr
								log:trace(
									"DevelopEditManager.applyMaskEdits candidate failed "
										.. tostring(key)
										.. " via "
										.. tostring(candidate)
										.. ": "
										.. tostring(setErr)
								)
							end
						end
						if not applied then
							appendWarning(
								warnings,
								"Mask adjustment '"
									.. tostring(key)
									.. "' could not be applied for "
									.. maskKind
									.. ": "
									.. tostring(lastErr or "unknown error")
							)
						end
					else
						appendWarning(
							warnings,
							"Mask adjustment '" .. tostring(key) .. "' is not currently supported and was ignored."
						)
					end
				end
			else
				log:trace(
					"DevelopEditManager.applyMaskEdits: LrDevelopController.setValue unavailable; relying on develop-settings fallback"
				)
			end

			-- Avoid clobbering controller-applied local slider values with a stale
			-- develop-settings snapshot. Only run the fallback when controller writes
			-- were not successfully applied.
			if controllerAppliedCount == 0 or not hasMaskContext then
				local fallbackOk, fallbackErr = applyMaskAdjustmentsViaDevelopSettings(maskKind, mask.adjustments or {})
				if not fallbackOk then
					appendWarning(
						warnings,
						"Mask adjustments for '"
							.. maskKind
							.. "' could not be persisted via develop settings: "
							.. tostring(fallbackErr)
					)
				end
			end
			logMaskedDevelopSnapshot("after_adjust_" .. maskKind)
		end)
		if not ok then
			appendWarning(warnings, "Mask '" .. maskKind .. "' could not be applied: " .. tostring(err))
			log:error(
				"DevelopEditManager.applyMaskEdits mask failed: " .. tostring(maskKind) .. " err=" .. tostring(err)
			)
		end
	end

	-- Leave masking UI so users return to normal Develop controls.
	if type(LrDevelopController.selectTool) == "function" then
		local okExit, exitErr = LrTasks.pcall(function()
			LrDevelopController.selectTool("loupe")
		end)
		if not okExit then
			log:trace("DevelopEditManager.applyMaskEdits: could not exit masking mode: " .. tostring(exitErr))
		end
	end

	log:trace("DevelopEditManager.applyMaskEdits: done")
	return true
end

function DevelopEditManager.showValidationDialog(context, photo, response, options)
	log:trace("DevelopEditManager.showValidationDialog: start")
	local recipe = getRecipeFromResponse(response)
	if not recipe then
		log:error("DevelopEditManager.showValidationDialog: no recipe in response")
		return "cancel", nil
	end

	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share
	local props = LrBinding.makePropertyTable(context)
	props.applyGlobal = next(recipe.global or {}) ~= nil or type(recipe.white_balance) == "table"
	props.applyMasks = (options and options.applyMasks ~= false) and ((recipe.masks and #recipe.masks > 0) or false)
	props.details = DevelopEditManager.formatRecipeDetails(response)
	props.engineTypeDisplay = recipe.engine_type or "Style Engine"
	props.baseProfileName = (recipe.global and recipe.global.profile) or "Adobe Standard"

	-- Style prediction metadata for the UI
	props.hasConfidence = response and response.confidence ~= nil
	local confVal = (response and response.confidence) or 0
	props.confidencePct = math.floor(confVal * 100)
	props.confidenceLabel = string.format("%d%%", props.confidencePct)

	local confColor = { 0.7, 0.7, 0.7 } -- gray
	local qualityText = "Low Style Match"
	if confVal >= 0.75 then
		confColor = { 0.2, 0.8, 0.2 } -- green
		qualityText = "Excellent Style Match"
	elseif confVal >= 0.50 then
		confColor = { 0.8, 0.8, 0.2 } -- yellow/gold
		qualityText = "Good Style Match"
	elseif confVal > 0 then
		confColor = { 0.8, 0.4, 0.1 } -- orange
		qualityText = "Weak Style Match"
	end
	props.qualityText = qualityText
	props.confColor = confColor

	local dialogView = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:row({
			f:static_text({
				title = photo:getFormattedMetadata("fileName") or "Photo",
				font = "<system_bold>",
			}),
			f:spacer({ fill_horizontal = 1 }),
			-- Confidence Badge
			f:row({
				visible = bind("hasConfidence"),
				f:static_text({
					title = "Match:",
				}),
				f:static_text({
					title = bind("confidenceLabel"),
					text_color = bind("confColor"),
					font = "<system_bold>",
				}),
				f:static_text({
					title = string.format(" (%s)", qualityText),
					size = "small",
				}),
			}),
		}),
		f:row({
			f:checkbox({ value = bind("applyGlobal") }),
			f:static_text({ title = LOC("$$$/LrGeniusAI/DevelopEdit/ApplyGlobal=Apply global develop settings") }),
		}),
		f:row({
			f:checkbox({
				value = bind("applyMasks"),
				enabled = (recipe.masks and #recipe.masks > 0) or false,
			}),
			f:static_text({ title = LOC("$$$/LrGeniusAI/DevelopEdit/ApplyMasks=Apply masks when possible") }),
		}),
		f:row({
			f:static_text({
				title = LOC("$$$/LrGeniusAI/DevelopEdit/EngineType=AI Engine:"),
				width = share("labelWidth"),
			}),
			f:static_text({
				title = bind("engineTypeDisplay"),
				font = "<system/bold>",
			}),
			f:spacer({ width = 10 }),
			f:static_text({
				title = LOC("$$$/LrGeniusAI/DevelopEdit/BaseProfile=Base Profile:"),
			}),
			f:static_text({
				title = bind("baseProfileName"),
				font = "<system/italic>",
			}),
		}),
		f:row({
			f:edit_field({
				value = bind("details"),
				width_in_chars = 70,
				height_in_lines = 22,
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/DevelopEdit/ReviewTitle=Review AI Lightroom Edit"),
		contents = dialogView,
		actionVerb = "Apply",
	})
	log:trace("DevelopEditManager.showValidationDialog: result=" .. tostring(result))

	if result == "ok" then
		return result, {
			applyGlobal = props.applyGlobal,
			applyMasks = props.applyMasks,
		}
	end
	return result, nil
end

function DevelopEditManager.applyRecipe(photo, response, options)
	log:trace("DevelopEditManager.applyRecipe: start")
	local recipe = getRecipeFromResponse(response)
	if not recipe then
		log:error("DevelopEditManager.applyRecipe: no recipe")
		return false, { "No edit recipe returned by the AI." }
	end

	local warnings = {}
	if type(recipe.warnings) == "table" then
		for _, warning in ipairs(recipe.warnings) do
			table.insert(warnings, tostring(warning))
		end
	end

	local applyGlobal = options == nil or options.applyGlobal ~= false
	local applyMasks = options ~= nil and options.applyMasks == true

	local globalApplied = true
	if applyGlobal then
		globalApplied = applyGlobalDevelopSettings(photo, recipe, warnings)
	end
	if applyMasks then
		applyMaskEdits(photo, recipe, warnings)
	end

	DevelopEditManager.persistEditRecipe(photo, response, warnings, "applied")
	log:trace(
		"DevelopEditManager.applyRecipe: done globalApplied="
			.. tostring(globalApplied)
			.. " warningsCount="
			.. tostring(#warnings)
	)
	return globalApplied, warnings
end
