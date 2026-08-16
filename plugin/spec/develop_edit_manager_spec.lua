-- Tests for the recipe -> Lightroom develop-settings mapping.
--
-- This is the seam where the edit path's value semantics live, and where it had
-- shipped a whole class of silent corruption: global recipe values were merged
-- *additively* onto the photo's current settings, even though every producer
-- emits absolute Lightroom slider values. For `Temp` that turned an as-shot
-- 5500 K plus a recommended 5600 K into 11100 K; for every control whose
-- Lightroom default is non-zero it was quietly wrong in the same way.

require("DevelopEditManager")

local internal = DevelopEditManager.internal
local mergeGlobalDevelopSettings = internal.mergeGlobalDevelopSettings
local buildDevelopSettings = internal.buildDevelopSettings
local normalizeDevelopValue = internal.normalizeDevelopValue

describe("DevelopEditManager global merge semantics", function()
	it("replaces white balance instead of adding to it", function()
		local current = { Temp = 5500, Tint = 10 }
		local merged = mergeGlobalDevelopSettings(current, { Temp = 5600, Tint = -4 })

		assert.are.equal(5600, merged.Temp)
		assert.are.equal(-4, merged.Tint)
	end)

	it("does not stack controls whose Lightroom default is non-zero", function()
		-- ColorNoiseReduction defaults to 25 on raw files, SharpenDetail to 25.
		-- Additive merging silently doubled both.
		local current = { ColorNoiseReduction = 25, SharpenDetail = 25, Sharpness = 40 }
		local merged = mergeGlobalDevelopSettings(current, {
			ColorNoiseReduction = 25,
			SharpenDetail = 25,
			Sharpness = 40,
		})

		assert.are.equal(25, merged.ColorNoiseReduction)
		assert.are.equal(25, merged.SharpenDetail)
		assert.are.equal(40, merged.Sharpness)
	end)

	it("replaces tone controls rather than double-counting an existing edit", function()
		-- The backend measures the RAW decode, not the photo's edited state, so a
		-- recipe value already accounts for the whole correction.
		local merged = mergeGlobalDevelopSettings({ Exposure2012 = 0.5 }, { Exposure2012 = 0.3 })

		assert.are.equal(0.3, merged.Exposure2012)
	end)

	it("keeps settings the recipe does not mention", function()
		local current = { CropLeft = 0.1, CropRight = 0.9, Exposure2012 = 0.5 }
		local merged = mergeGlobalDevelopSettings(current, { Contrast2012 = 12 })

		assert.are.equal(0.1, merged.CropLeft)
		assert.are.equal(0.9, merged.CropRight)
		assert.are.equal(0.5, merged.Exposure2012)
		assert.are.equal(12, merged.Contrast2012)
	end)

	it("clamps out-of-range values to the Lightroom slider bounds", function()
		local merged = mergeGlobalDevelopSettings({}, { Exposure2012 = 99, Contrast2012 = -500 })

		assert.are.equal(5, merged.Exposure2012)
		assert.are.equal(-100, merged.Contrast2012)
	end)

	it("keeps a nil current-settings table usable", function()
		local merged = mergeGlobalDevelopSettings(nil, { Temp = 4800 })

		assert.are.equal(4800, merged.Temp)
	end)
end)

describe("DevelopEditManager value normalization", function()
	it("wraps hue angles instead of clamping them", function()
		assert.are.equal(10, normalizeDevelopValue("SplitToningShadowHue", 370))
		assert.are.equal(350, normalizeDevelopValue("SplitToningShadowHue", -10))
	end)

	it("leaves unknown keys and non-numbers untouched", function()
		assert.are.equal("Adobe Color", normalizeDevelopValue("CameraProfile", "Adobe Color"))
		assert.are.equal(1234, normalizeDevelopValue("SomeUnknownKey", 1234))
	end)
end)

describe("DevelopEditManager recipe mapping", function()
	it("maps canonical recipe fields onto Lightroom develop keys", function()
		local settings = buildDevelopSettings({
			global = {
				exposure = 0.4,
				contrast = 12,
				temperature = 5200,
				clarity = 8,
				color_noise_reduction = 30,
			},
		}, {})

		assert.are.equal(0.4, settings.Exposure2012)
		assert.are.equal(12, settings.Contrast2012)
		assert.are.equal(5200, settings.Temp)
		assert.are.equal(8, settings.Clarity2012)
		assert.are.equal(30, settings.ColorNoiseReduction)
	end)

	it("writes the camera profile under the key Lightroom actually uses", function()
		local settings = buildDevelopSettings({ global = { profile = "Camera Neutral" } }, {})

		assert.are.equal("Camera Neutral", settings.CameraProfile)
		assert.is_nil(settings.CameraConfig)
	end)

	it("expands HSL channels into per-colour develop keys", function()
		local settings = buildDevelopSettings({
			global = { hsl = { blue = { hue = -5, saturation = 10, luminance = -20 } } },
		}, {})

		assert.are.equal(-5, settings.HueAdjustmentBlue)
		assert.are.equal(10, settings.SaturationAdjustmentBlue)
		assert.are.equal(-20, settings.LuminanceAdjustmentBlue)
	end)

	it("returns an empty mapping for a recipe without globals", function()
		assert.are.same({}, buildDevelopSettings({}, {}))
	end)
end)

describe("DevelopEditManager.formatRecipeDetails", function()
	it("renders the recipe instead of always reporting none available", function()
		-- Regression: this read an undeclared global `recipe`, which was whitelisted
		-- in .luacheckrc and therefore always nil, so the review dialog's detail
		-- pane was permanently empty and users approved edits blind.
		local details = DevelopEditManager.formatRecipeDetails({
			edit = {
				summary = "Warm, gentle contrast",
				global = { exposure = 0.3, contrast = 10 },
				masks = {},
				warnings = {},
			},
		})

		assert.is_truthy(details:find("Warm, gentle contrast", 1, true))
		assert.is_truthy(details:find("exposure", 1, true))
		assert.is_falsy(details:find("No edit recipe available", 1, true))
	end)

	it("reports masks and warnings that the recipe carries", function()
		local details = DevelopEditManager.formatRecipeDetails({
			edit = {
				summary = "Sky recovery",
				global = {},
				masks = { { kind = "sky", adjustments = { exposure = -0.4, clarity = 5 } } },
				warnings = { "Highlights were already clipped" },
			},
		})

		assert.is_truthy(details:find("sky", 1, true))
		assert.is_truthy(details:find("Highlights were already clipped", 1, true))
	end)

	it("still reports nothing available when there is no recipe", function()
		assert.is_truthy(DevelopEditManager.formatRecipeDetails(nil):find("No edit recipe available", 1, true))
		assert.is_truthy(DevelopEditManager.formatRecipeDetails({}):find("No edit recipe available", 1, true))
	end)
end)

-- Lightroom's Temp is Kelvin for raw and a relative -100..100 for rendered
-- files. Every clamp below used the Kelvin bounds regardless of the file, which
-- turned a sensible relative `+10` into 2000 — maximum cool — and a Kelvin
-- value on a JPEG into +100, maximum warm.
describe("DevelopEditManager white balance scale", function()
	it("clamps a raw photo's Temp on the Kelvin scale", function()
		assert.are.equal(2000, normalizeDevelopValue("Temp", 1500, true))
		assert.are.equal(50000, normalizeDevelopValue("Temp", 90000, true))
		assert.are.equal(6200, normalizeDevelopValue("Temp", 6200, true))
	end)

	it("clamps a non-raw photo's Temp on the relative scale", function()
		assert.are.equal(10, normalizeDevelopValue("Temp", 10, false))
		assert.are.equal(-100, normalizeDevelopValue("Temp", -180, false))
	end)

	it("treats an unknown raw status as raw, as it always did", function()
		assert.are.equal(6200, normalizeDevelopValue("Temp", 6200, nil))
	end)

	it("leaves every other key alone whatever the file is", function()
		assert.are.equal(50, normalizeDevelopValue("Contrast2012", 50, false))
		assert.are.equal(100, normalizeDevelopValue("Contrast2012", 400, false))
	end)

	it("drops a Kelvin value for a non-raw photo instead of clamping it", function()
		-- Clamping 6200 into -100..100 gives +100: a hard orange cast. Leaving
		-- the photo's own white balance untouched is the lesser harm.
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 6200, contrast = 10 } }, warnings, false)

		assert.is_nil(settings.Temp)
		assert.are.equal(10, settings.Contrast2012)
		assert.are.equal(1, #warnings)
		assert.is_truthy(warnings[1]:find("Kelvin", 1, true))
	end)

	it("keeps a relative value for a non-raw photo", function()
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 12 } }, warnings, false)

		assert.are.equal(12, settings.Temp)
		assert.are.equal(0, #warnings)
	end)

	it("keeps Kelvin for a raw photo", function()
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 6200 } }, warnings, true)

		assert.are.equal(6200, settings.Temp)
		assert.are.equal(0, #warnings)
	end)

	it("does not strip the photo's own Temp when it drops the recipe's", function()
		-- The merge starts from the current settings, so a dropped key has to be
		-- absent from the recipe rather than nil'd during the merge.
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 6200 } }, warnings, false)
		local merged = mergeGlobalDevelopSettings({ Temp = 25, Tint = 3 }, settings, false)

		assert.are.equal(25, merged.Temp)
		assert.are.equal(3, merged.Tint)
	end)
end)
