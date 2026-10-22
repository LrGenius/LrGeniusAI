-- Headless tests for the pure parts of AgentCommands.lua: how agent arguments
-- become catalog queries, keyword paths and develop values, how replies are
-- made JSON-safe, and how the dispatcher refuses what the user turned off.
-- The parts that need the real catalog run inside Lightroom (PluginTests.lua).

require("AgentCommands")

describe("AgentCommands.splitPath", function()
	it("splits and trims keyword paths", function()
		assert.same({ "Places", "Hamburg" }, AgentCommands.splitPath(" Places > Hamburg "))
		assert.same({ "Dog" }, AgentCommands.splitPath("Dog"))
	end)

	it("drops empty parts", function()
		assert.same({ "A", "B" }, AgentCommands.splitPath("A >  > B >"))
		assert.same({}, AgentCommands.splitPath(nil))
	end)
end)

describe("AgentCommands.buildSearchDesc", function()
	it("returns nil when nothing maps to findPhotos (dates and metadata are filtered in Lua)", function()
		assert.is_nil(AgentCommands.buildSearchDesc({ captured_after = "2025-01-01" }))
		assert.is_nil(AgentCommands.buildSearchDesc({}))
		-- findPhotos found nothing for these in LrC 15.6; buildMetadataFilter has them.
		assert.is_nil(
			AgentCommands.buildSearchDesc({ rating_min = 3, flag = "pick", color_label = "red", file_type = "tiff" })
		)
	end)

	it("uses a single criterion as it is", function()
		assert.same(
			{ criteria = "all", operation = "all", value = "Bearbeitet" },
			AgentCommands.buildSearchDesc({ text = "Bearbeitet" })
		)
	end)

	it("intersects several criteria", function()
		local desc = AgentCommands.buildSearchDesc({ text = "sunset", camera = "R5", rating_min = 4 })
		assert.equals("intersect", desc.combine)
		assert.equals(2, #desc)
		assert.same({ criteria = "camera", operation = "all", value = "R5" }, desc[2])
	end)

	it("maps the edited switch", function()
		assert.equals("isFalse", AgentCommands.buildSearchDesc({ edited = false }).operation)
		assert.equals("isTrue", AgentCommands.buildSearchDesc({ edited = true }).operation)
	end)

	it("matches any of several keywords", function()
		local desc = AgentCommands.buildSearchDesc({ keywords = { "dog", "cat" } })
		assert.equals("union", desc.combine)
		assert.equals("dog", desc[1].value)
		assert.equals("words", desc[2].operation)
	end)
end)

describe("AgentCommands.buildMetadataFilter", function()
	it("returns nil when no metadata criterion is given", function()
		assert.is_nil(AgentCommands.buildMetadataFilter({}))
		assert.is_nil(AgentCommands.buildMetadataFilter({ text = "x", edited = true }))
	end)

	it("filters by rating, unrated photos counting as 0 stars", function()
		local keys, match = AgentCommands.buildMetadataFilter({ rating_min = 1 })
		assert.same({ "rating" }, keys)
		assert.is_true(match({ rating = 4 }))
		assert.is_true(match({ rating = 1 }))
		assert.is_false(match({ rating = 0 }))
		assert.is_false(match({}))
		assert.is_false(match(nil))

		local _, unrated = AgentCommands.buildMetadataFilter({ rating_max = 0 })
		assert.is_true(unrated({}))
		assert.is_false(unrated({ rating = 2 }))

		local _, range = AgentCommands.buildMetadataFilter({ rating_min = 3, rating_max = 4 })
		assert.is_true(range({ rating = 3 }))
		assert.is_false(range({ rating = 5 }))
	end)

	it("filters by flag", function()
		local _, pick = AgentCommands.buildMetadataFilter({ flag = "Pick" })
		assert.is_true(pick({ pickStatus = 1 }))
		assert.is_false(pick({ pickStatus = 0 }))
		local _, unflagged = AgentCommands.buildMetadataFilter({ flag = "unflagged" })
		assert.is_true(unflagged({}))
		assert.is_false(unflagged({ pickStatus = -1 }))
	end)

	it("filters by colour label, treating gray, white and missing as none", function()
		local _, red = AgentCommands.buildMetadataFilter({ color_label = "red" })
		assert.is_true(red({ colorNameForLabel = "red" }))
		assert.is_false(red({ colorNameForLabel = "blue" }))
		local _, none = AgentCommands.buildMetadataFilter({ color_label = "none" })
		assert.is_true(none({ colorNameForLabel = "gray" }))
		assert.is_true(none({ colorNameForLabel = "white" }))
		assert.is_true(none({}))
		assert.is_false(none({ colorNameForLabel = "purple" }))
	end)

	it("filters by file type", function()
		local keys, tiff = AgentCommands.buildMetadataFilter({ file_type = "tiff" })
		assert.same({ "fileFormat" }, keys)
		assert.is_true(tiff({ fileFormat = "TIFF" }))
		assert.is_false(tiff({ fileFormat = "RAW" }))
		local _, jpeg = AgentCommands.buildMetadataFilter({ file_type = "jpeg" })
		assert.is_true(jpeg({ fileFormat = "JPG" }))
	end)

	it("needs every criterion to match and fetches each key once", function()
		local keys, match =
			AgentCommands.buildMetadataFilter({ rating_min = 2, rating_max = 5, flag = "pick", color_label = "red" })
		assert.same({ "rating", "pickStatus", "colorNameForLabel" }, keys)
		assert.is_true(match({ rating = 4, pickStatus = 1, colorNameForLabel = "red" }))
		assert.is_false(match({ rating = 4, pickStatus = 0, colorNameForLabel = "red" }))
	end)

	it("rejects values it cannot map", function()
		assert.has_error(function()
			AgentCommands.buildMetadataFilter({ flag = "maybe" })
		end)
		assert.has_error(function()
			AgentCommands.buildMetadataFilter({ color_label = "pink" })
		end)
		assert.has_error(function()
			AgentCommands.buildMetadataFilter({ file_type = "bmp" })
		end)
	end)
end)

describe("AgentCommands.labelName", function()
	it("keeps the five label colours and calls everything else none", function()
		assert.equals("red", AgentCommands.labelName("Red"))
		assert.equals("none", AgentCommands.labelName("gray"))
		assert.equals("none", AgentCommands.labelName("white"))
		assert.equals("none", AgentCommands.labelName(""))
		assert.equals("none", AgentCommands.labelName(nil))
	end)
end)

describe("AgentCommands.parseDay", function()
	it("reads ISO days only", function()
		assert.same({ 2025, 7, 14 }, { AgentCommands.parseDay("2025-07-14") })
		assert.is_nil(AgentCommands.parseDay("14.07.2025"))
		assert.is_nil(AgentCommands.parseDay(nil))
	end)
end)

describe("AgentCommands.developValue", function()
	it("sets absolute values as given", function()
		assert.equals(0.5, AgentCommands.developValue(1.2, 0.5, false))
		assert.equals("Auto", AgentCommands.developValue("As Shot", "Auto", false))
	end)

	it("adds relative values to the current one", function()
		assert.equals(1.5, AgentCommands.developValue(1.25, 0.25, true))
	end)

	it("refuses a relative change without numbers", function()
		local value, reason = AgentCommands.developValue(nil, 0.3, true)
		assert.is_nil(value)
		assert.truthy(reason)
		assert.is_nil((AgentCommands.developValue(1, "x", true)))
	end)
end)

describe("AgentCommands.toJsonSafe", function()
	it("passes plain values through", function()
		assert.same({ a = 1, b = { "x", true } }, AgentCommands.toJsonSafe({ a = 1, b = { "x", true } }))
	end)

	it("cuts cycles and turns numbers JSON has no form for into text", function()
		local t = {}
		t.self = t
		assert.equals("<cycle>", AgentCommands.toJsonSafe(t).self)
		assert.equals("inf", AgentCommands.toJsonSafe(math.huge))
	end)

	it("describes functions instead of failing", function()
		assert.is_string(AgentCommands.toJsonSafe(print))
	end)

	it("caps big tables", function()
		local big = {}
		for i = 1, 600 do
			big[i] = i
		end
		local out = AgentCommands.toJsonSafe(big)
		assert.truthy(out["..."])
	end)
end)

describe("AgentCommands.describe", function()
	it("says what the agent is doing, with a photo count", function()
		assert.equals(
			"Agent: updating photos (3 photos)",
			AgentCommands.describe({ action = "update_photos", args = { photo_ids = { "a", "b", "c" } } })
		)
		assert.equals("Agent: rendering a preview", AgentCommands.describe({ action = "render_preview", args = {} }))
		assert.equals("Agent: something_new", AgentCommands.describe({ action = "something_new" }))
	end)
end)

describe("AgentCommands.run", function()
	local env = { allowChanges = false, devTools = false, cache = AgentCommands.newCache() }

	it("answers unknown commands instead of raising", function()
		local reply = AgentCommands.run({ action = "nope", args = {} }, env)
		assert.is_false(reply.ok)
		assert.truthy(reply.error:find("nope", 1, true))
	end)

	it("refuses changes on a read-only bridge", function()
		local reply = AgentCommands.run({ action = "update_photos", args = {} }, env)
		assert.equals("changes", reply.refused)
		assert.is_true(AgentCommands.changesPhotos("apply_develop_settings"))
		assert.is_false(AgentCommands.changesPhotos("show_photos"))
	end)

	it("refuses developer commands unless they are on", function()
		local reply = AgentCommands.run({ action = "eval_lua", args = { code = "return 1" } }, env)
		assert.equals("dev_tools", reply.refused)
	end)

	it("runs developer code when allowed and returns the value and output", function()
		local dev = { allowChanges = false, devTools = true, cache = AgentCommands.newCache() }
		local reply = AgentCommands.run({ action = "eval_lua", args = { code = "print('hi', 2) return 40 + 2" } }, dev)
		assert.is_true(reply.ok, tostring(reply.error))
		assert.equals(42, reply.result.value)
		assert.same({ "hi\t2" }, reply.result.output)
	end)

	it("reports a failing handler with its message, without the file prefix", function()
		local dev = { allowChanges = false, devTools = true, cache = AgentCommands.newCache() }
		local reply = AgentCommands.run({ action = "eval_lua", args = { code = "error('boom')" } }, dev)
		assert.is_false(reply.ok)
		assert.truthy(reply.error:find("boom", 1, true))
	end)
end)

describe("AgentCommands.resolvePhotos", function()
	it("looks photos up by Lightroom UUID when global IDs are off", function()
		local savedPrefs, savedApp = _G.prefs, _G.LrApplication
		_G.prefs = { useGlobalPhotoId = false }
		local photo = { localIdentifier = 7 }
		_G.LrApplication = {
			activeCatalog = function()
				return {
					findPhotoByUuid = function(_, uuid)
						return uuid == "u-1" and photo or nil
					end,
				}
			end,
		}
		local photos, missing = AgentCommands.resolvePhotos({ cache = AgentCommands.newCache() }, { "u-1", "u-2" })
		_G.prefs, _G.LrApplication = savedPrefs, savedApp
		assert.equals(1, #photos)
		assert.equals(photo, photos[1])
		assert.same({ "u-2" }, missing)
	end)
end)
