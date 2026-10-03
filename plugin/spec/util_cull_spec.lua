-- Unit tests for the culling helpers in Util.lua.
--
-- Which tasks the prep pass sends decides whether a preset can judge the
-- moment at all, and dropping a warning is how a degraded cull looks clean.
-- Both are pure logic, so they are tested headless here.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.cullPrepTasks", function()
	it("keeps the fast path for the default preset", function()
		assert.are.same({ "cull" }, Util.cullPrepTasks("default"))
		assert.are.same({ "cull" }, Util.cullPrepTasks(nil))
		assert.are.same({ "cull" }, Util.cullPrepTasks(""))
	end)

	it("adds the embedding for every preset that judges the moment", function()
		-- Without an embedding the backend has nothing to judge emotion or
		-- action from, and ranks sports and events on sharpness alone.
		for _, preset in ipairs({ "sports", "event", "portrait", "street" }) do
			assert.is_true(Util.cullPresetJudgesMoment(preset), preset)
			assert.are.same({ "cull", "embeddings" }, Util.cullPrepTasks(preset), preset)
		end
	end)

	it("asks for the image model as well as the face model", function()
		assert.are.same({ "clip", "face" }, Util.requiredModelFamilies(Util.cullPrepTasks("sports")))
		assert.are.same({ "face" }, Util.requiredModelFamilies(Util.cullPrepTasks("default")))
	end)
end)

describe("Util.responseWarnings", function()
	it("prefers the list and removes duplicates", function()
		local list = Util.responseWarnings({
			warning = "a\n\nb",
			warnings = { "a", "b", "a", "" },
		})
		assert.are.same({ "a", "b" }, list)
	end)

	it("falls back to the single string of an older backend", function()
		assert.are.same({ "only" }, Util.responseWarnings({ warning = "only" }))
	end)

	it("returns an empty list when there is nothing to say", function()
		assert.are.same({}, Util.responseWarnings({ warnings = {} }))
		assert.are.same({}, Util.responseWarnings({ status = "success" }))
		assert.are.same({}, Util.responseWarnings(nil))
	end)
end)

describe("Util.formatWarningList", function()
	it("shows a handful and counts the rest", function()
		local text = Util.formatWarningList({ "1", "2", "3", "4" }, 2)
		assert.are.equal("1\n\n2\n\n... and 2 more warnings", text)
	end)

	it("shows everything when it fits", function()
		assert.are.equal("1\n\n2", Util.formatWarningList({ "1", "2" }))
	end)

	it("returns nil for no warnings", function()
		assert.is_nil(Util.formatWarningList({}))
		assert.is_nil(Util.formatWarningList(nil))
	end)
end)
