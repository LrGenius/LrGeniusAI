-- A style-engine response with every `global` field the backend sends since
-- step 1f (detail, vignette and grain sliders, HSL, colour grading shadows/
-- highlights/balance, the parametric curve splits), fed through the recipe
-- mapping.
--
-- The point of 1f is that the *installed* plug-in already applies all of
-- these, so this spec is written against the module's long-standing seams
-- only (formatRecipeDetails, internal.buildDevelopSettings,
-- internal.mergeGlobalDevelopSettings, internal.normalizeDevelopValue). It
-- also runs unchanged against the release on `main`, which is how the
-- compatibility claim was checked.

require("DevelopEditManager")

local internal = DevelopEditManager.internal

-- The shape of POST /v1/edit/style's answer (see the contract test
-- `style_edit_sends_every_global_field_the_installed_plugin_applies`).
local function styleResponse()
	return {
		status = "success",
		engine = "style",
		confidence = 0.82,
		matched_examples = 3,
		matched_filenames = { "a.cr3", "b.cr3", "c.cr3" },
		warnings = {},
		guardrail_reasons = {},
		guardrail_explanations = {},
		edit = {
			summary = "Style: Moody / Matched 3 of 12 examples (confidence 82%)",
			global = {
				exposure = 0.35,
				contrast = 12,
				highlights = -40,
				shadows = 25,
				whites = 10,
				blacks = -8,
				texture = 5,
				clarity = 8,
				dehaze = 3,
				vibrance = 10,
				saturation = -5,
				sharpening = 40,
				sharpen_radius = 1.2,
				sharpen_detail = 30,
				sharpen_masking = 15,
				noise_reduction = 20,
				noise_reduction_detail = 55,
				noise_reduction_contrast = 5,
				color_noise_reduction = 25,
				color_noise_reduction_detail = 50,
				color_noise_reduction_smoothness = 60,
				vignette = -12,
				vignette_midpoint = 40,
				vignette_roundness = -10,
				vignette_feather = 70,
				vignette_highlights = 20,
				grain = 15,
				grain_size = 30,
				grain_roughness = 60,
				hsl = {
					red = { hue = 0, saturation = 0, luminance = 0 },
					orange = { hue = 4, saturation = -6, luminance = 12 },
					yellow = { hue = 0, saturation = -10, luminance = 0 },
					green = { hue = 20, saturation = -30, luminance = -10 },
					aqua = { hue = -8, saturation = -15, luminance = 0 },
					blue = { hue = -5, saturation = -20, luminance = -12 },
					purple = { hue = 0, saturation = 0, luminance = 0 },
					magenta = { hue = 0, saturation = -5, luminance = 0 },
				},
				color_grading = {
					shadows = { hue = 220, saturation = 15 },
					highlights = { hue = 40, saturation = 20 },
					balance = -10,
				},
				tone_curve = {
					highlights = -10,
					lights = 5,
					darks = 8,
					shadows = -6,
					shadow_split = 20,
					midtone_split = 45,
					highlight_split = 70,
				},
			},
			masks = {},
			warnings = {},
			white_balance = { mode = "Custom", family = "raw", temperature = 5450, tint = 10 },
		},
	}
end

-- Recipe field -> Lightroom key and value, for everything 1f added.
local EXPECTED = {
	SharpenRadius = 1.2,
	SharpenDetail = 30,
	SharpenEdgeMasking = 15,
	LuminanceNoiseReductionDetail = 55,
	LuminanceNoiseReductionContrast = 5,
	ColorNoiseReductionDetail = 50,
	ColorNoiseReductionSmoothness = 60,
	PostCropVignetteMidpoint = 40,
	PostCropVignetteRoundness = -10,
	PostCropVignetteFeather = 70,
	PostCropVignetteHighlightContrast = 20,
	GrainSize = 30,
	GrainFrequency = 60,
	HueAdjustmentGreen = 20,
	SaturationAdjustmentAqua = -15,
	LuminanceAdjustmentBlue = -12,
	HueAdjustmentPurple = 0,
	SplitToningShadowHue = 220,
	SplitToningShadowSaturation = 15,
	SplitToningHighlightHue = 40,
	SplitToningHighlightSaturation = 20,
	SplitToningBalance = -10,
	ParametricShadowSplit = 20,
	ParametricMidtoneSplit = 45,
	ParametricHighlightSplit = 70,
}

-- The registry's UI range of each key 1f sends (lrg-develop registry, the
-- same ranges the backend's schema drift test pins). The plug-in must not
-- clamp anything inside them.
local REGISTRY_RANGES = {
	SharpenRadius = { 0.5, 3.0 },
	SharpenDetail = { 0, 100 },
	SharpenEdgeMasking = { 0, 100 },
	LuminanceNoiseReductionDetail = { 0, 100 },
	LuminanceNoiseReductionContrast = { 0, 100 },
	ColorNoiseReductionDetail = { 0, 100 },
	ColorNoiseReductionSmoothness = { 0, 100 },
	PostCropVignetteMidpoint = { 0, 100 },
	PostCropVignetteRoundness = { -100, 100 },
	PostCropVignetteFeather = { 0, 100 },
	PostCropVignetteHighlightContrast = { 0, 100 },
	GrainSize = { 0, 100 },
	GrainFrequency = { 0, 100 },
	SplitToningShadowSaturation = { 0, 100 },
	SplitToningHighlightSaturation = { 0, 100 },
	SplitToningBalance = { -100, 100 },
	ParametricShadowSplit = { 0, 100 },
	ParametricMidtoneSplit = { 0, 100 },
	ParametricHighlightSplit = { 0, 100 },
}
for _, colour in ipairs({ "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta" }) do
	for _, slider in ipairs({ "Hue", "Saturation", "Luminance" }) do
		REGISTRY_RANGES[slider .. "Adjustment" .. colour] = { -100, 100 }
	end
end

describe("a 1f style response", function()
	it("maps every new field onto its Lightroom key without a warning", function()
		local warnings = {}
		local recipe = styleResponse().edit
		-- The third argument is the photo's white-balance family ("raw") on
		-- this branch and `isRaw` on older releases; "raw" is truthy for both.
		local settings = internal.buildDevelopSettings(recipe, warnings, "raw")

		assert.are.same({}, warnings)
		for key, value in pairs(EXPECTED) do
			assert.are.equal(value, settings[key], key)
		end
		assert.is_true(settings.EnableSplitToning)
		-- 24 HSL keys, all of them.
		local hslCount = 0
		for key in pairs(settings) do
			if
				key:match("^HueAdjustment")
				or key:match("^SaturationAdjustment")
				or key:match("^LuminanceAdjustment")
			then
				hslCount = hslCount + 1
			end
		end
		assert.are.equal(24, hslCount)
		-- Nothing outside what 1f intends: no colour-grading keys the plug-in
		-- cannot map, and no `Temp`.
		for key in pairs(settings) do
			assert.is_falsy(key:match("^ColorGrade"), key)
			assert.are_not.equal("Temp", key)
		end
	end)

	it("merges onto the photo without clamping or wrapping any value", function()
		local settings = internal.buildDevelopSettings(styleResponse().edit, {}, "raw")
		local merged = internal.mergeGlobalDevelopSettings({ HueAdjustmentRed = 30, Exposure2012 = 1 }, settings)

		for key, value in pairs(EXPECTED) do
			assert.are.equal(value, merged[key], key)
		end
		assert.are.equal(0, merged.HueAdjustmentRed, "the recipe's absolute 0 replaces the photo's 30")
	end)

	it("clamps nothing inside the registry's range of each key", function()
		for key, range in pairs(REGISTRY_RANGES) do
			assert.are.equal(range[1], internal.normalizeDevelopValue(key, range[1]), key .. " min")
			assert.are.equal(range[2], internal.normalizeDevelopValue(key, range[2]), key .. " max")
		end
		-- Hue angles: 0..359 pass as they are (360 is 0 on the circle).
		for _, key in ipairs({ "SplitToningShadowHue", "SplitToningHighlightHue" }) do
			assert.are.equal(0, internal.normalizeDevelopValue(key, 0))
			assert.are.equal(359, internal.normalizeDevelopValue(key, 359))
			assert.are.equal(0, internal.normalizeDevelopValue(key, 360))
		end
	end)

	it("shows up in the review dialog without table addresses or new warnings", function()
		local details = DevelopEditManager.formatRecipeDetails(styleResponse())

		assert.is_falsy(details:find("table: 0x", 1, true), details)
		assert.is_truthy(details:find("- hsl: 8 channel(s)", 1, true), details)
		assert.is_truthy(details:find("- color_grading: enabled", 1, true), details)
		assert.is_truthy(details:find("- tone_curve: enabled", 1, true), details)
		assert.is_truthy(details:find("- sharpen_radius: 1.2", 1, true), details)
		assert.is_truthy(details:find("- grain_roughness: 60", 1, true), details)
		assert.is_truthy(details:find("- vignette_highlights: 20", 1, true), details)
		local section = details:sub((details:find("\nWarnings", 1, true)))
		assert.are.equal("\nWarnings\n- none", section)
	end)
end)
