-- Tests for the recipe -> Lightroom develop-settings mapping.
--
-- This is the seam where the edit path's value semantics live, and where it had
-- shipped a whole class of silent corruption: global recipe values were merged
-- *additively* onto the photo's current settings, even though every producer
-- emits absolute Lightroom slider values. For the white balance that turned an
-- as-shot 5500 K plus a recommended 5600 K into 11100 K; for every control
-- whose Lightroom default is non-zero it was quietly wrong in the same way.

require("DevelopEditManager")

local internal = DevelopEditManager.internal
local mergeGlobalDevelopSettings = internal.mergeGlobalDevelopSettings
local buildDevelopSettings = internal.buildDevelopSettings
local normalizeDevelopValue = internal.normalizeDevelopValue

describe("DevelopEditManager global merge semantics", function()
	it("replaces white balance instead of adding to it", function()
		local current = { Temperature = 5500, Tint = 10 }
		local merged = mergeGlobalDevelopSettings(current, { Temperature = 5600, Tint = -4 })

		assert.are.equal(5600, merged.Temperature)
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
		local merged = mergeGlobalDevelopSettings(nil, { Temperature = 4800 })

		assert.are.equal(4800, merged.Temperature)
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
				clarity = 8,
				color_noise_reduction = 30,
			},
		}, {})

		assert.are.equal(0.4, settings.Exposure2012)
		assert.are.equal(12, settings.Contrast2012)
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

	it("lists the response's warnings next to the recipe's", function()
		-- The style engine reports why white balance was left out in the
		-- response's `warnings`; the recipe's own list is empty then.
		local wb = "White balance was not transferred: your training examples are raw files and this photo is not."
		local details = DevelopEditManager.formatRecipeDetails({
			engine = "style",
			edit = { summary = "s", global = { exposure = 0.2 }, masks = {}, warnings = {} },
			warnings = { wb },
		})

		local section = details:sub((details:find("Warnings", 1, true)))
		assert.is_truthy(section:find(wb, 1, true))
		assert.is_falsy(section:find("- none", 1, true))
	end)

	it("lists each line of an old backend's joined warning once", function()
		local details = DevelopEditManager.formatRecipeDetails({
			edit = { summary = "s", global = { exposure = 0.2 }, masks = {}, warnings = { "a" } },
			warning = "a\nb",
		})

		local section = details:sub((details:find("Warnings", 1, true)))
		assert.are.equal("Warnings\n- a\n- b", section)
	end)

	it("still reports nothing available when there is no recipe", function()
		assert.is_truthy(DevelopEditManager.formatRecipeDetails(nil):find("No edit recipe available", 1, true))
		assert.is_truthy(DevelopEditManager.formatRecipeDetails({}):find("No edit recipe available", 1, true))
	end)
end)

-- White balance lives in two families of develop keys: a raw file stores
-- Kelvin in `Temperature` plus an absolute `Tint` (-150..150); a rendered file
-- (and a DNG converted from one) stores offsets in `IncrementalTemperature` /
-- `IncrementalTint` (-100..100). The plug-in used to write `Temp` for both,
-- which is not a key Lightroom reports, and clamped it on whichever scale
-- `Util.isRawPhoto` guessed.
describe("DevelopEditManager.whiteBalanceFamily", function()
	local family = DevelopEditManager.whiteBalanceFamily

	it("reads a raw file's family from its Temperature key", function()
		assert.are.equal("raw", family({ Temperature = 5500, Tint = 10 }, true))
	end)

	it("reads a rendered file's family from its IncrementalTemperature key", function()
		assert.are.equal("non_raw", family({ IncrementalTemperature = 0, IncrementalTint = 0 }, false))
	end)

	it("trusts the settings over the file format for a DNG converted from a JPEG", function()
		-- Util.isRawPhoto counts every DNG as raw; its white balance is not.
		assert.are.equal("non_raw", family({ IncrementalTemperature = 0 }, true))
	end)

	it("falls back to the file format when neither key is present", function()
		assert.are.equal("raw", family({}, true))
		assert.are.equal("non_raw", family({}, false))
		assert.are.equal("raw", family(nil, true))
	end)

	it("returns nil when neither source knows", function()
		assert.is_nil(family({}, nil))
		assert.is_nil(family(nil, nil))
	end)
end)

describe("DevelopEditManager white balance mapping", function()
	local function wbRecipe(wb, global)
		return { summary = "x", global = global or {}, masks = {}, white_balance = wb }
	end

	it("writes a raw Custom white balance to Temperature/Tint", function()
		local warnings = {}
		local settings = buildDevelopSettings(
			wbRecipe({ mode = "Custom", family = "raw", temperature = 5600, tint = 5 }),
			warnings,
			"raw"
		)

		assert.are.equal(5600, settings.Temperature)
		assert.are.equal(5, settings.Tint)
		assert.are.equal("Custom", settings.WhiteBalance)
		assert.is_nil(settings.IncrementalTemperature)
		assert.is_nil(settings.IncrementalTint)
		assert.is_nil(settings.Temp)
		assert.are.equal(0, #warnings)
	end)

	it("writes a non-raw Custom white balance to the Incremental keys only", function()
		local warnings = {}
		local settings = buildDevelopSettings(
			wbRecipe({ mode = "Custom", family = "non_raw", temperature = 12, tint = -3 }),
			warnings,
			"non_raw"
		)

		assert.are.equal(12, settings.IncrementalTemperature)
		assert.are.equal(-3, settings.IncrementalTint)
		assert.are.equal("Custom", settings.WhiteBalance)
		assert.is_nil(settings.Temperature)
		assert.is_nil(settings.Tint)
		assert.is_nil(settings.Temp)
		assert.are.equal(0, #warnings)
	end)

	it("maps the old global.temperature/tint form on a raw photo", function()
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 6200, tint = 4 } }, warnings, "raw")

		assert.are.equal(6200, settings.Temperature)
		assert.are.equal(4, settings.Tint)
		assert.are.equal("Custom", settings.WhiteBalance)
		assert.is_nil(settings.Temp)
		assert.are.equal(0, #warnings)
	end)

	it("maps the old global.temperature/tint form on a non-raw photo", function()
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 12, tint = -2 } }, warnings, "non_raw")

		assert.are.equal(12, settings.IncrementalTemperature)
		assert.are.equal(-2, settings.IncrementalTint)
		assert.are.equal("Custom", settings.WhiteBalance)
		assert.is_nil(settings.Temperature)
		assert.is_nil(settings.Tint)
		assert.are.equal(0, #warnings)
	end)

	it("maps the LLM's global.white_balance alias object like the old form", function()
		local settings = buildDevelopSettings(
			{ global = { white_balance = { temperature = 6200, tint = 13 } } },
			{},
			"raw"
		)

		assert.are.equal(6200, settings.Temperature)
		assert.are.equal(13, settings.Tint)
	end)

	it("prefers the recipe-level white balance over the old global form", function()
		local settings = buildDevelopSettings(
			wbRecipe({ mode = "Custom", family = "raw", temperature = 5600, tint = 5 }, { temperature = 9000 }),
			{},
			"raw"
		)

		assert.are.equal(5600, settings.Temperature)
	end)

	it("writes the Incremental keys for a DNG converted from a JPEG", function()
		-- The develop settings say non-raw although the file format says DNG.
		local family = DevelopEditManager.whiteBalanceFamily({ IncrementalTemperature = 0 }, true)
		local settings = buildDevelopSettings({ global = { temperature = 8, tint = 1 } }, {}, family)

		assert.are.equal(8, settings.IncrementalTemperature)
		assert.are.equal(1, settings.IncrementalTint)
		assert.is_nil(settings.Temperature)
	end)

	it("clamps each family on its own scale", function()
		local raw = mergeGlobalDevelopSettings(
			{},
			buildDevelopSettings(
				wbRecipe({ mode = "Custom", family = "raw", temperature = 90000, tint = 400 }),
				{},
				"raw"
			)
		)
		assert.are.equal(50000, raw.Temperature)
		assert.are.equal(150, raw.Tint)

		local rendered = mergeGlobalDevelopSettings(
			{},
			buildDevelopSettings(
				wbRecipe({ mode = "Custom", family = "non_raw", temperature = -180, tint = 140 }),
				{},
				"non_raw"
			)
		)
		assert.are.equal(-100, rendered.IncrementalTemperature)
		assert.are.equal(100, rendered.IncrementalTint)

		assert.are.equal(2000, normalizeDevelopValue("Temperature", 1500))
		assert.are.equal(-150, normalizeDevelopValue("Tint", -200))
	end)

	it("drops a Kelvin value aimed at a non-raw photo, with a warning", function()
		-- Clamping 6200 into -100..100 gives +100: a hard orange cast.
		local warnings = {}
		local settings =
			buildDevelopSettings({ global = { temperature = 6200, tint = 20, contrast = 10 } }, warnings, "non_raw")

		assert.is_nil(settings.IncrementalTemperature)
		assert.is_nil(settings.IncrementalTint)
		assert.is_nil(settings.WhiteBalance)
		assert.are.equal(10, settings.Contrast2012)
		assert.are.equal(1, #warnings)
		assert.is_truthy(warnings[1]:find("Kelvin", 1, true))
	end)

	it("drops a relative value aimed at a raw photo, with a warning", function()
		-- The Kelvin clamp would turn +10 into 2000 K, maximum cool.
		local warnings = {}
		local settings = buildDevelopSettings({ global = { temperature = 10 } }, warnings, "raw")

		assert.is_nil(settings.Temperature)
		assert.is_nil(settings.WhiteBalance)
		assert.are.equal(1, #warnings)
	end)

	it("drops a white balance learned from the other family, with a warning", function()
		local warnings = {}
		local settings = buildDevelopSettings(
			wbRecipe({ mode = "Custom", family = "raw", temperature = 5600, tint = 5 }),
			warnings,
			"non_raw"
		)

		assert.is_nil(settings.Temperature)
		assert.is_nil(settings.IncrementalTemperature)
		assert.is_nil(settings.WhiteBalance)
		assert.are.equal(1, #warnings)
		assert.is_truthy(warnings[1]:find("raw files", 1, true))
	end)

	it("writes no white balance and warns when the photo's family is unknown", function()
		local warnings = {}
		local settings = buildDevelopSettings(
			wbRecipe({ mode = "Custom", family = "raw", temperature = 5600, tint = 5 }),
			warnings,
			nil
		)
		assert.is_nil(settings.Temperature)
		assert.is_nil(settings.Tint)
		assert.is_nil(settings.WhiteBalance)
		assert.are.equal(1, #warnings)

		local oldWarnings = {}
		local oldForm = buildDevelopSettings({ global = { temperature = 5600 } }, oldWarnings, nil)
		assert.is_nil(oldForm.Temperature)
		assert.is_nil(oldForm.Temp)
		assert.are.equal(1, #oldWarnings)
	end)

	it("warns about an unrecognised family instead of guessing", function()
		local warnings = {}
		local settings =
			buildDevelopSettings(wbRecipe({ mode = "Custom", family = "linear", temperature = 5600 }), warnings, "raw")

		assert.is_nil(settings.Temperature)
		assert.are.equal(1, #warnings)
	end)

	it("reports a named mode instead of applying it blind", function()
		-- Not sent by the backend until experiment E1 shows how Lightroom
		-- treats a mode without numbers.
		local warnings = {}
		local settings = buildDevelopSettings(wbRecipe({ mode = "Daylight", family = "raw" }), warnings, "raw")

		assert.is_nil(settings.WhiteBalance)
		assert.are.equal(1, #warnings)
		assert.is_truthy(warnings[1]:find("Daylight", 1, true))
	end)

	it("writes nothing and warns about nothing when the recipe has no white balance", function()
		local warnings = {}
		local settings = buildDevelopSettings({ global = { exposure = 0.2 } }, warnings, nil)

		assert.is_nil(settings.WhiteBalance)
		assert.is_nil(settings.Temperature)
		assert.are.equal(0, #warnings)
	end)

	it("keeps the photo's own white balance when the recipe has none", function()
		local settings = buildDevelopSettings({ global = { contrast = 10 } }, {}, "raw")
		local merged = mergeGlobalDevelopSettings({ Temperature = 4300, Tint = 7, WhiteBalance = "As Shot" }, settings)

		assert.are.equal(4300, merged.Temperature)
		assert.are.equal(7, merged.Tint)
		assert.are.equal("As Shot", merged.WhiteBalance)
		assert.are.equal(10, merged.Contrast2012)
	end)

	it("keeps the photo's own white balance when it drops the recipe's", function()
		-- The merge starts from the current settings, so a dropped key has to
		-- be absent from the recipe rather than nil'd during the merge.
		local settings = buildDevelopSettings({ global = { temperature = 6200 } }, {}, "non_raw")
		local merged = mergeGlobalDevelopSettings(
			{ IncrementalTemperature = 25, IncrementalTint = 3, WhiteBalance = "As Shot" },
			settings
		)

		assert.are.equal(25, merged.IncrementalTemperature)
		assert.are.equal(3, merged.IncrementalTint)
		assert.are.equal("As Shot", merged.WhiteBalance)
	end)
end)

describe("DevelopEditManager white balance display", function()
	it("shows a raw white balance in Kelvin with a signed tint", function()
		assert.are.equal(
			"Custom 5600 K, tint +5",
			DevelopEditManager.formatWhiteBalance({ mode = "Custom", family = "raw", temperature = 5600, tint = 5 })
		)
	end)

	it("shows a non-raw white balance as signed offsets", function()
		assert.are.equal(
			"Custom, temperature +12, tint -3",
			DevelopEditManager.formatWhiteBalance({ mode = "Custom", family = "non_raw", temperature = 12, tint = -3 })
		)
	end)

	it("shows a mode without numbers by its name", function()
		assert.are.equal("Auto", DevelopEditManager.formatWhiteBalance({ mode = "Auto", family = "raw" }))
	end)

	it("never prints a table address for a style-engine response", function()
		local details = DevelopEditManager.formatRecipeDetails({
			status = "success",
			engine = "style",
			warnings = { "White balance was not transferred" },
			edit = {
				summary = "Warm",
				global = { exposure = 0.2 },
				masks = {},
				warnings = {},
				white_balance = { mode = "Custom", family = "raw", temperature = 5600, tint = 5 },
			},
		})

		assert.is_falsy(details:find("table: 0x", 1, true))
		assert.is_truthy(details:find("White balance: Custom 5600 K, tint +5", 1, true))
		assert.is_truthy(details:find("exposure", 1, true))
	end)

	it("never prints a table address for the LLM's white_balance alias or unknown groups", function()
		local details = DevelopEditManager.formatRecipeDetails({
			edit = {
				summary = "LLM",
				global = {
					white_balance = { temperature = 6200, tint = 13 },
					some_future_group = { a = 1, b = 2 },
				},
				masks = {},
				warnings = {},
			},
		})

		assert.is_falsy(details:find("table: 0x", 1, true))
		assert.is_truthy(details:find("white_balance: temperature 6200, tint +13", 1, true))
		assert.is_truthy(details:find("some_future_group: 2 setting(s)", 1, true))
	end)
end)
