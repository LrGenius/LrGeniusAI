-- AI Edit's optional refinement loop: how much of the style-engine recipe does
-- this photo actually want?
--
-- The style engine hands back one recipe, the photographer's own average edit
-- for frames like this one. It cannot tell whether that average suits *this*
-- frame. The loop asks the aesthetic scorer instead: apply the recipe, render
-- the photo, score the render, and — while a gentler version scores clearly
-- better — step the strength down and try again.
--
-- The search only ever moves between 0 and the recipe as it came back. It never
-- goes above 1.0: the backend's guardrails already pulled the recipe in to what
-- the frame can take, and a scorer that liked more contrast would be arguing
-- with the one thing in the pipeline that has looked at the clipping. What the
-- scorer decides is how much of the photographer's style to keep, not what the
-- style is.
--
-- The scorer is a general model of what photographs look good, not of what this
-- photographer likes, so it only ever breaks ties *downwards*: a candidate has
-- to beat the stronger one by MIN_GAIN to win, and on a tie the stronger edit
-- (closer to the user's own style) stays.

AestheticRefine = {}

-- Tried in this order. The full recipe first, because that is the edit the user
-- asked for and every other candidate is only a reason to depart from it.
AestheticRefine.STRENGTHS = { 1.0, 0.75, 0.5, 0.25 }

-- The scorer reports two decimals; a difference below this is noise, not a
-- better photograph. Scores run 1..9.
AestheticRefine.MIN_GAIN = 0.05

---
-- Walks the strengths in order and keeps the best-scoring one.
--
-- Stops at the first candidate that does not clearly beat the best so far, on
-- the assumption that scores fall away once they have started to: it costs a
-- render per strength tried, and each render is seconds of Lightroom time.
--
-- @param evaluate function `(strength) -> score|nil, err` Applies, renders, scores.
-- @param options table|nil `strengths`, `minGain`, `shouldStop` (function -> boolean,
--   asked before every candidate after the first).
-- @return table `{ best = {strength, score}|nil, trials = {{strength, score}...},
--   stopped = "no_gain"|"exhausted"|"canceled"|"error", error = string|nil }`.
--   `best` is nil only when the very first candidate could not be evaluated.
---
function AestheticRefine.search(evaluate, options)
	options = options or {}
	local strengths = options.strengths or AestheticRefine.STRENGTHS
	local minGain = options.minGain or AestheticRefine.MIN_GAIN

	local result = { best = nil, trials = {}, stopped = "exhausted", error = nil }
	for index, strength in ipairs(strengths) do
		if index > 1 and options.shouldStop and options.shouldStop() then
			result.stopped = "canceled"
			break
		end

		local score, err = evaluate(strength)
		if type(score) ~= "number" then
			result.stopped = "error"
			result.error = tostring(err or "no score")
			break
		end

		local trial = { strength = strength, score = score }
		table.insert(result.trials, trial)
		if result.best == nil or score > result.best.score + minGain then
			result.best = trial
		else
			result.stopped = "no_gain"
			break
		end
	end
	return result
end

local function percent(strength)
	return string.format("%d%%", math.floor(strength * 100 + 0.5))
end

---
-- What a finished search means for the user.
--
-- @param result table From `search`.
-- @param baselineScore number The unedited photo's score.
-- @return table `{ line = string, warning = string|nil, strength = number }`.
--   `line` is for the log and the run summary. `warning` is set when the user
--   should look at the photo: the edit scores below the original even at the
--   best strength found, or the search could not finish. `strength` is the one
--   to apply — the whole recipe when no candidate could be scored at all.
---
function AestheticRefine.describe(result, baselineScore)
	local best = result.best
	if not best then
		local reason = tostring(result.error)
		return {
			strength = 1.0,
			line = "not refined",
			warning = "the aesthetic score was unavailable (" .. reason .. "), so the whole recipe was applied",
		}
	end

	local line
	if best.strength == 1.0 then
		line = string.format("full strength, aesthetic %.2f (original %.2f)", best.score, baselineScore)
	else
		line = string.format(
			"strength %s, aesthetic %.2f (full recipe %.2f, original %.2f)",
			percent(best.strength),
			best.score,
			result.trials[1].score,
			baselineScore
		)
	end

	local warning = nil
	if result.stopped == "error" then
		warning = "the refinement stopped early (" .. tostring(result.error) .. "); kept the best result so far"
	end
	if best.score < baselineScore - AestheticRefine.MIN_GAIN then
		local below = string.format(
			"it scores lower than the original (%.2f against %.2f) even at %s — worth a look",
			best.score,
			baselineScore,
			percent(best.strength)
		)
		warning = warning and (warning .. "; " .. below) or below
	end
	return { strength = best.strength, line = line, warning = warning }
end

-- Renders the photo as it stands and scores it. The temporary file is always
-- removed, whatever the score did.
local function renderAndScore(photo)
	local path = SearchIndexAPI.exportPhotoForIndexing(photo)
	if not path then
		return nil, "the photo could not be rendered"
	end
	local ok, score, err = LrTasks.pcall(function()
		return SearchIndexAPI.scoreEditedPhoto(path)
	end)
	LrTasks.pcall(function()
		if LrFileUtils.exists(path) then
			LrFileUtils.delete(path)
		end
	end)
	if not ok then
		return nil, tostring(score)
	end
	if type(score) ~= "number" then
		return nil, Util.userFacingError(tostring(err))
	end
	return score, nil
end

---
-- Refines and applies one photo's edit.
--
-- Leaves the photo carrying the chosen strength, through the same `applyRecipe`
-- a plain run uses, so masks and the persisted recipe behave exactly as they do
-- without refinement.
--
-- Probing edits the photo for real, one Edit History entry per candidate, which
-- is why the dialog offers the virtual copy next to this option.
--
-- @param args table `photo`, `response` (the style edit), `baselineScore`
--   (the unedited photo's score), `applyOptions` (as for applyRecipe),
--   `isCanceled` (function -> boolean, optional), `onTrial` (function(strength),
--   optional, for a progress caption).
-- @return boolean|nil applied, table warnings, table|nil info Where `applied` is
--   nil the photo was not touched and the caller should apply the plain recipe.
--   `info` is `describe`'s result plus `trials`.
---
function AestheticRefine.run(args)
	local photo = args.photo
	local baseline, baselineErr = DevelopEditManager.captureBaseline(photo)
	if not baseline then
		local why = "not refined: could not read the photo's develop settings (" .. tostring(baselineErr) .. ")"
		return nil, { why }, nil
	end

	local function tryStrength(strength)
		if args.onTrial then
			args.onTrial(strength)
		end
		local response = args.response
		local applied, applyWarnings = DevelopEditManager.applyGlobalAtStrength(photo, response, strength, baseline)
		if not applied then
			local why = tostring(applyWarnings[1])
			return nil, "could not apply the edit at " .. percent(strength) .. " to try it: " .. why
		end
		local score, err = renderAndScore(photo)
		log:trace("AestheticRefine: " .. percent(strength) .. " scored " .. tostring(score or err))
		return score, err
	end

	local result = AestheticRefine.search(tryStrength, { shouldStop = args.isCanceled })

	local info = AestheticRefine.describe(result, args.baselineScore)
	info.trials = result.trials
	log:info("AestheticRefine: " .. info.line)

	local applyOptions = {}
	for key, value in pairs(args.applyOptions or {}) do
		applyOptions[key] = value
	end
	applyOptions.strength = info.strength
	applyOptions.baseline = baseline

	local applied, warnings = DevelopEditManager.applyRecipe(photo, args.response, applyOptions)
	if info.warning then
		warnings = warnings or {}
		table.insert(warnings, info.warning)
	end
	return applied, warnings, info
end
