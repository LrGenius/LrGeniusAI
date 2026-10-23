-- Tests for AI Edit's aesthetic refinement loop: the search over strengths and
-- what it tells the user. The parts that render and score photos need
-- Lightroom; what is decided from the scores does not.

require("AestheticRefine")

-- An `evaluate` that answers from a table of strength -> score, and fails for
-- a strength the table does not have.
local function scores(byStrength)
	local asked = {}
	local evaluate = function(strength)
		table.insert(asked, strength)
		local score = byStrength[strength]
		if score == nil then
			return nil, "no score for " .. tostring(strength)
		end
		return score, nil
	end
	return evaluate, asked
end

describe("AestheticRefine.search", function()
	it("never tries more than the whole recipe", function()
		-- The backend's guardrails already pulled the recipe in to what the frame
		-- can take; a scorer that liked more contrast must not undo that.
		for _, strength in ipairs(AestheticRefine.STRENGTHS) do
			assert.is_true(strength <= 1.0)
			assert.is_true(strength > 0)
		end
		assert.are.equal(1.0, AestheticRefine.STRENGTHS[1])
	end)

	it("keeps the full recipe when a gentler one is not clearly better", function()
		local evaluate = scores({ [1.0] = 6.0, [0.75] = 6.02 })
		local result = AestheticRefine.search(evaluate)

		assert.are.equal(1.0, result.best.strength)
		assert.are.equal("no_gain", result.stopped)
		assert.are.equal(2, #result.trials)
	end)

	it("lets a tie go to the stronger edit, which is closer to the user's own style", function()
		local evaluate = scores({ [1.0] = 6.0, [0.75] = 6.0 })

		assert.are.equal(1.0, AestheticRefine.search(evaluate).best.strength)
	end)

	it("steps down while each step clearly improves, and stops at the first that does not", function()
		local evaluate, asked = scores({ [1.0] = 5.0, [0.75] = 5.5, [0.5] = 5.9, [0.25] = 5.93 })
		local result = AestheticRefine.search(evaluate)

		assert.are.equal(0.5, result.best.strength)
		assert.are.equal(5.9, result.best.score)
		assert.are.equal("no_gain", result.stopped)
		assert.are.same({ 1.0, 0.75, 0.5, 0.25 }, asked)
	end)

	it("does not look further once a step has failed to improve", function()
		-- 0.5 would score best here, but the search stops at 0.75: every extra
		-- strength is a render, and scores are assumed to fall away once they have.
		local evaluate, asked = scores({ [1.0] = 5.0, [0.75] = 4.9, [0.5] = 9.0, [0.25] = 9.0 })
		local result = AestheticRefine.search(evaluate)

		assert.are.equal(1.0, result.best.strength)
		assert.are.same({ 1.0, 0.75 }, asked)
	end)

	it("reaches the gentlest strength when every step improves", function()
		local evaluate = scores({ [1.0] = 4.0, [0.75] = 5.0, [0.5] = 6.0, [0.25] = 7.0 })
		local result = AestheticRefine.search(evaluate)

		assert.are.equal(0.25, result.best.strength)
		assert.are.equal("exhausted", result.stopped)
	end)

	it("reports a first candidate that cannot be scored, with nothing chosen", function()
		local result = AestheticRefine.search(scores({}))

		assert.is_nil(result.best)
		assert.are.equal("error", result.stopped)
		assert.are.equal("no score for 1", result.error)
	end)

	it("keeps the best result so far when a later candidate fails", function()
		local result = AestheticRefine.search(scores({ [1.0] = 5.0, [0.75] = 5.5 }))

		assert.are.equal(0.75, result.best.strength)
		assert.are.equal("error", result.stopped)
		assert.is_truthy(result.error)
	end)

	it("stops when asked to, after at least the full recipe has been scored", function()
		local evaluate, asked = scores({ [1.0] = 5.0, [0.75] = 6.0 })
		local result = AestheticRefine.search(evaluate, {
			shouldStop = function()
				return true
			end,
		})

		assert.are.equal("canceled", result.stopped)
		assert.are.same({ 1.0 }, asked)
		assert.are.equal(1.0, result.best.strength)
	end)

	it("honours a different strength list and minimum gain", function()
		local evaluate = scores({ [1.0] = 5.0, [0.5] = 5.2 })
		local result = AestheticRefine.search(evaluate, { strengths = { 1.0, 0.5 }, minGain = 0.1 })

		assert.are.equal(0.5, result.best.strength)
		assert.are.equal("exhausted", result.stopped)
	end)
end)

describe("AestheticRefine.describe", function()
	it("applies the whole recipe, and says why, when nothing could be scored", function()
		local info = AestheticRefine.describe({ best = nil, trials = {}, error = "model not loaded" }, 5.0)

		assert.are.equal(1.0, info.strength)
		assert.is_truthy(info.warning:find("model not loaded", 1, true))
		assert.is_truthy(info.warning:find("whole recipe", 1, true))
	end)

	it("reports the strength chosen and what it scored against the alternatives", function()
		local result = {
			best = { strength = 0.5, score = 6.1 },
			trials = {
				{ strength = 1.0, score = 5.2 },
				{ strength = 0.75, score = 5.7 },
				{ strength = 0.5, score = 6.1 },
			},
			stopped = "exhausted",
		}
		local info = AestheticRefine.describe(result, 5.0)

		assert.are.equal(0.5, info.strength)
		assert.is_truthy(info.line:find("50%", 1, true))
		assert.is_truthy(info.line:find("6.10", 1, true))
		assert.is_truthy(info.line:find("5.20", 1, true))
		assert.is_truthy(info.line:find("5.00", 1, true))
		assert.is_nil(info.warning)
	end)

	it("calls the full recipe by that name", function()
		local result = {
			best = { strength = 1.0, score = 6.0 },
			trials = { { strength = 1.0, score = 6.0 } },
			stopped = "no_gain",
		}

		assert.is_truthy(AestheticRefine.describe(result, 5.0).line:find("full strength", 1, true))
	end)

	it("warns when the edit scores below the original even at the best strength found", function()
		local result = {
			best = { strength = 0.25, score = 4.0 },
			trials = { { strength = 1.0, score = 3.0 }, { strength = 0.25, score = 4.0 } },
			stopped = "exhausted",
		}
		local info = AestheticRefine.describe(result, 5.0)

		assert.is_truthy(info.warning:find("lower than the original", 1, true))
		assert.is_truthy(info.warning:find("25%", 1, true))
	end)

	it("does not warn about a dip within the scorer's noise", function()
		local result = {
			best = { strength = 1.0, score = 4.97 },
			trials = { { strength = 1.0, score = 4.97 } },
			stopped = "no_gain",
		}

		assert.is_nil(AestheticRefine.describe(result, 5.0).warning)
	end)

	it("warns when the search stopped early on an error, and still names the strength kept", function()
		local result = {
			best = { strength = 0.75, score = 5.5 },
			trials = { { strength = 1.0, score = 5.0 }, { strength = 0.75, score = 5.5 } },
			stopped = "error",
			error = "export failed",
		}
		local info = AestheticRefine.describe(result, 5.0)

		assert.are.equal(0.75, info.strength)
		assert.is_truthy(info.warning:find("export failed", 1, true))
	end)
end)

describe("AestheticRefine.run", function()
	local realManager, realApi, realFileUtils
	local applied, deleted, exported

	-- Lightroom and the backend, replaced by doubles that record what they were
	-- asked: `scoreByStrength` is what the scorer will say about the render made
	-- after each candidate went on.
	local function install(scoreByStrength, options)
		options = options or {}
		applied = {}
		deleted = {}
		exported = 0
		local current = nil

		_G.DevelopEditManager = {
			captureBaseline = function()
				if options.noBaseline then
					return nil, "no settings"
				end
				return { Exposure2012 = 0 }, nil
			end,
			applyGlobalAtStrength = function(_, _, strength, baseline)
				current = strength
				table.insert(applied, { strength = strength, baseline = baseline })
				return true, {}
			end,
			applyRecipe = function(_, _, applyOptions)
				table.insert(applied, { final = true, options = applyOptions })
				return true, { "from applyRecipe" }
			end,
		}
		_G.SearchIndexAPI = {
			exportPhotoForIndexing = function()
				exported = exported + 1
				return "/tmp/render" .. exported .. ".jpg"
			end,
			scoreEditedPhoto = function()
				local score = scoreByStrength[current]
				if score == nil then
					return nil, "scorer is down"
				end
				return score, nil
			end,
		}
		_G.LrFileUtils = {
			exists = function()
				return true
			end,
			delete = function(path)
				table.insert(deleted, path)
			end,
		}
	end

	local function run(baselineScore)
		return AestheticRefine.run({ photo = {}, response = {}, baselineScore = baselineScore, applyOptions = {} })
	end

	before_each(function()
		realManager, realApi, realFileUtils = _G.DevelopEditManager, _G.SearchIndexAPI, _G.LrFileUtils
	end)

	after_each(function()
		_G.DevelopEditManager, _G.SearchIndexAPI, _G.LrFileUtils = realManager, realApi, realFileUtils
	end)

	it("leaves the photo at the strength that scored best, measured from the original settings", function()
		install({ [1.0] = 5.0, [0.75] = 5.6, [0.5] = 5.62 })
		local ok, warnings, info = AestheticRefine.run({
			photo = {},
			response = {},
			baselineScore = 4.0,
			applyOptions = { applyGlobal = true, applyMasks = false },
		})

		assert.is_true(ok)
		assert.are.equal(0.75, info.strength)
		local final = applied[#applied]
		assert.is_true(final.final)
		assert.are.equal(0.75, final.options.strength)
		assert.are.equal(0, final.options.baseline.Exposure2012)
		-- The caller's own options come through untouched.
		assert.are.equal(true, final.options.applyGlobal)
		assert.are.same({ "from applyRecipe" }, warnings)
	end)

	it("removes every temporary render", function()
		install({ [1.0] = 5.0, [0.75] = 5.6, [0.5] = 5.62 })
		run(4.0)

		assert.are.equal(exported, #deleted)
		assert.is_true(exported >= 2)
	end)

	it("applies the whole recipe and says why when the scorer is unavailable", function()
		install({})
		local ok, warnings, info = run(4.0)

		assert.is_true(ok)
		assert.are.equal(1.0, applied[#applied].options.strength)
		assert.are.equal(1.0, info.strength)
		assert.is_truthy(warnings[#warnings]:find("scorer is down", 1, true))
	end)

	it("reports the edit as untouched when the original settings cannot be read", function()
		install({}, { noBaseline = true })
		local ok, warnings, info = run(4.0)

		assert.is_nil(ok)
		assert.is_nil(info)
		assert.is_truthy(warnings[1]:find("not refined", 1, true))
		assert.are.equal(0, #applied)
	end)

	it("warns when even the chosen edit scores below the original", function()
		install({ [1.0] = 3.0, [0.75] = 3.0 })
		local _, warnings = run(5.0)

		assert.is_truthy(warnings[#warnings]:find("lower than the original", 1, true))
	end)
end)
