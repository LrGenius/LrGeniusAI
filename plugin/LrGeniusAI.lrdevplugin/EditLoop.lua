-- AI Edit's retoucher loop, Lightroom side.
--
-- The backend holds the conversation with the AI model (`/v1/edit/sessions`,
-- server-rs/crates/lrg-api/src/edit_loop.rs). Lightroom does what only it can:
-- apply each step's develop table, compute the AI masks in it, render the
-- result and hand the render back, until the model says the photo is done.
--
-- Every step the backend sends is the complete edit measured from the photo's
-- starting point, not a change on top of the previous step. So each round is
-- applied as: the starting value of every key an earlier round set and this
-- one does not, then this round's values. Masks follow the same rule by
-- `CorrectionSyncID`: the backend derives the id from the mask's target, so
-- "LrGenius · Sky" in round 2 replaces round 1's, and the photo's own masks
-- (the ones it had before) are kept as Lightroom gave them to us — passing
-- them back raw keeps their computed state, which the backend's writer could
-- not.
--
-- Masks are written into the table and computed by `photo:updateAISettings()`
-- (Lightroom 15.3 and later), the route the develop-settings experiments
-- settled (docs/wiki/Dev-AI-Edit-XMP-Findings.md, E2).

EditLoop = {}

-- Long edge of every render the model sees, the original's included, so the
-- measurements of one compare with the next. The backend scales anything
-- larger down to this anyway.
EditLoop.RENDER_EDGE = 1536

-- How long a round waits for Lightroom to compute its AI masks.
EditLoop.MASK_WAIT_SECONDS = 30

-- How long a round waits for a photo another AI operation holds.
EditLoop.AVAILABLE_WAIT_SECONDS = 30

local MASKS_KEY = "MaskGroupBasedCorrections"

---
-- Whether this Lightroom can write AI masks through develop settings:
-- `photo:updateAISettings()` arrived with SDK 15.3.
-- @param versionTable table|nil `LrApplication.versionTable()`.
-- @return boolean
---
function EditLoop.supportsMasks(versionTable)
	if type(versionTable) ~= "table" then
		return false
	end
	local major = tonumber(versionTable.major) or 0
	local minor = tonumber(versionTable.minor) or 0
	return major > 15 or (major == 15 and minor >= 3)
end

---
-- The table to apply for one round. Pure.
--
-- @param baseline table The photo's `getDevelopSettings()` before the loop.
-- @param roundTable table The step's `develop_settings`.
-- @param state table `{ touched = {key = true}, ownSyncIds = {id = true} }`,
--   carried from round to round and updated here: the keys and the mask ids
--   any round so far has written.
-- @return table What `applyDevelopSettings` gets.
---
function EditLoop.composeApply(baseline, roundTable, state)
	baseline = baseline or {}
	roundTable = roundTable or {}
	state.touched = state.touched or {}
	state.ownSyncIds = state.ownSyncIds or {}

	local out = {}
	for key in pairs(state.touched) do
		if roundTable[key] == nil and baseline[key] ~= nil then
			out[key] = baseline[key]
		end
	end
	for key, value in pairs(roundTable) do
		if key ~= MASKS_KEY then
			out[key] = value
			state.touched[key] = true
		end
	end

	local roundMasks = type(roundTable[MASKS_KEY]) == "table" and roundTable[MASKS_KEY] or {}
	for _, correction in ipairs(roundMasks) do
		if type(correction) == "table" and correction.CorrectionSyncID then
			state.ownSyncIds[correction.CorrectionSyncID] = true
		end
	end
	if next(state.ownSyncIds) ~= nil then
		local merged = {}
		local own = type(baseline[MASKS_KEY]) == "table" and baseline[MASKS_KEY] or {}
		for _, correction in ipairs(own) do
			if type(correction) ~= "table" or not state.ownSyncIds[correction.CorrectionSyncID] then
				table.insert(merged, correction)
			end
		end
		for _, correction in ipairs(roundMasks) do
			table.insert(merged, correction)
		end
		out[MASKS_KEY] = merged
	end
	return out
end

---
-- The table that takes the photo back to where it started: the starting
-- value of every key the loop wrote, and the photo's own masks. Pure.
-- @param baseline table
-- @param state table As for composeApply.
-- @return table
---
function EditLoop.composeRevert(baseline, state)
	return EditLoop.composeApply(baseline, {}, state)
end

---
-- What Lightroom has made of one AI mask correction. Pure.
--
-- Read the way the experiments read it: a component with a `MaskDigest` has
-- its bitmap, one with a non-zero `ErrorReason` was computed and found
-- nothing, anything else is still only the definition we wrote.
-- @param correction table|nil One entry of `MaskGroupBasedCorrections`.
-- @return string "computed", "nothing_found", "pending" or "missing".
---
function EditLoop.maskState(correction)
	if type(correction) ~= "table" then
		return "missing"
	end
	local pending, failed, computed = false, false, false
	for _, mask in ipairs(correction.CorrectionMasks or {}) do
		if type(mask) == "table" and mask.What == "Mask/Image" then
			if mask.MaskDigest ~= nil and mask.MaskDigest ~= "" then
				computed = true
			elseif tonumber(mask.ErrorReason) ~= nil and tonumber(mask.ErrorReason) ~= 0 then
				failed = true
			else
				pending = true
			end
		end
	end
	if pending then
		return "pending"
	end
	if failed and not computed then
		return "nothing_found"
	end
	return "computed"
end

---
-- The state of each of this round's masks on the photo. Pure.
-- @param settings table The photo's `getDevelopSettings()` after applying.
-- @param roundTable table The step's `develop_settings`.
-- @return table `{ { name = "LrGenius · Sky", state = "computed" }, ... }`
---
function EditLoop.roundMaskStates(settings, roundTable)
	local byId = {}
	local onPhoto = type(settings) == "table" and settings[MASKS_KEY] or nil
	for _, correction in ipairs(type(onPhoto) == "table" and onPhoto or {}) do
		if type(correction) == "table" and correction.CorrectionSyncID then
			byId[correction.CorrectionSyncID] = correction
		end
	end
	local states = {}
	local roundMasks = type(roundTable) == "table" and roundTable[MASKS_KEY] or nil
	for _, correction in ipairs(type(roundMasks) == "table" and roundMasks or {}) do
		table.insert(states, {
			name = correction.CorrectionName or "a mask",
			state = EditLoop.maskState(byId[correction.CorrectionSyncID]),
		})
	end
	return states
end

---
-- What the model should hear about this round's masks. Pure.
-- @param states table From roundMaskStates.
-- @return table List of sentences.
---
function EditLoop.maskNotes(states)
	local notes = {}
	for _, entry in ipairs(states or {}) do
		local name = tostring(entry.name):gsub("^LrGenius · ", "")
		if entry.state == "nothing_found" then
			table.insert(
				notes,
				"Lightroom's " .. name .. " mask found nothing in this photo, so its adjustments do nothing."
			)
		elseif entry.state == "pending" or entry.state == "missing" then
			table.insert(
				notes,
				"Lightroom had not finished computing the " .. name .. " mask when this render was made."
			)
		end
	end
	return notes
end

-- Lightroom-facing from here on.

local function sleepUntil(seconds, check)
	local waited = 0
	while waited < seconds do
		if check() then
			return true
		end
		LrTasks.sleep(0.5)
		waited = waited + 0.5
	end
	return check()
end

local function readSettings(photo)
	local ok, settingsOrErr = LrTasks.pcall(function()
		return photo:getDevelopSettings()
	end)
	if ok and type(settingsOrErr) == "table" then
		return settingsOrErr, nil
	end
	return nil, tostring(settingsOrErr)
end

local function waitUntilAvailable(photo)
	return sleepUntil(EditLoop.AVAILABLE_WAIT_SECONDS, function()
		local ok, available = LrTasks.pcall(function()
			return photo:isAvailableForEditing()
		end)
		-- Older Lightroom has no such call and no such lock.
		return not ok or available ~= false
	end)
end

---
-- Applies a table in one history step and, when it carries AI masks, has
-- Lightroom compute them.
-- @return boolean ok, string|nil error, table notes for the model
---
local function applyTable(photo, tbl, updateAI, historyName, roundTable)
	if not waitUntilAvailable(photo) then
		return false, "the photo stayed locked by another AI operation in Lightroom", {}
	end
	local catalog = LrApplication.activeCatalog()
	local ok, statusOrErr = LrTasks.pcall(function()
		return catalog:withWriteAccessDo(historyName, function()
			photo:applyDevelopSettings(tbl, historyName)
		end, Defaults.catalogWriteAccessOptions)
	end)
	if not ok then
		return false, tostring(statusOrErr), {}
	end
	if statusOrErr == "aborted" then
		return false, "Lightroom did not grant catalog write access in time", {}
	end
	if not updateAI then
		return true, nil, {}
	end

	local okUpdate, updateErr = LrTasks.pcall(function()
		catalog:withWriteAccessDo("AI Edit: compute masks", function()
			photo:updateAISettings()
		end, Defaults.catalogWriteAccessOptions)
	end)
	if not okUpdate then
		log:warn("EditLoop: updateAISettings failed: " .. tostring(updateErr))
		return true,
			nil,
			{ "Lightroom could not compute this round's AI masks, so they do nothing (" .. tostring(updateErr) .. ")." }
	end
	local states
	sleepUntil(EditLoop.MASK_WAIT_SECONDS, function()
		local settings = readSettings(photo)
		states = EditLoop.roundMaskStates(settings, roundTable)
		for _, entry in ipairs(states) do
			if entry.state == "pending" or entry.state == "missing" then
				return false
			end
		end
		return true
	end)
	return true, nil, EditLoop.maskNotes(states)
end

local function render(photo)
	local paths, failures = SearchIndexAPI.exportPhotosForIndexing({ photo }, {
		LR_size_maxHeight = EditLoop.RENDER_EDGE,
	})
	local path = paths[photo]
	if path then
		return path, nil
	end
	return nil, tostring(failures[photo] or "Lightroom could not render the photo")
end

local function removeFile(path)
	LrTasks.pcall(function()
		if path and LrFileUtils.exists(path) then
			LrFileUtils.delete(path)
		end
	end)
end

local function appendAll(list, items)
	for _, item in ipairs(items or {}) do
		if type(item) == "string" and item ~= "" then
			table.insert(list, item)
		end
	end
end

---
-- Runs the loop on one photo.
--
-- Leaves the photo carrying the round the session ended on. When nothing
-- could be applied the photo is untouched; when the session broke off after
-- a round was applied, that round stays and the error is a warning.
--
-- @param args table
--   `photo`, `photoId`,
--   `fields` (the session's request fields, see SearchIndexAPI.startEditSession),
--   `styleRecipe` (table|nil, the style engine's `edit` for round 0),
--   `isCanceled` (function -> boolean), `onProgress` (function(caption)).
-- @return table `{ applied = boolean, error = string|nil, canceled = boolean,
--   keptRound = n|nil, verdict = string|nil, rounds = { {round, source,
--   assessment} ... }, recipe = table|nil, warnings = {...},
--   guardrailExplanations = {...}, baseline = table, state = table,
--   inputTokens, outputTokens }`. `baseline` and `state` are what
--   EditLoop.revert needs.
---
function EditLoop.run(args)
	local photo = args.photo
	local result = {
		applied = false,
		canceled = false,
		rounds = {},
		warnings = {},
		guardrailExplanations = {},
		state = { touched = {}, ownSyncIds = {} },
	}
	local progress = args.onProgress or function() end
	local isCanceled = args.isCanceled or function()
		return false
	end

	local baseline, baselineErr = readSettings(photo)
	if not baseline then
		result.error = "could not read the photo's develop settings (" .. tostring(baselineErr) .. ")"
		return result
	end
	result.baseline = baseline

	progress("rendering the original")
	local originalPath, renderErr = render(photo)
	if not originalPath then
		result.error = "could not render the photo: " .. renderErr
		return result
	end

	local fields = {}
	for key, value in pairs(args.fields or {}) do
		fields[key] = value
	end
	fields.develop_settings = JSON:encode(baseline)
	if args.styleRecipe then
		fields.style_recipe = JSON:encode(args.styleRecipe)
	end

	progress("asking the AI model for a first edit")
	local okCall, ok, step = LrTasks.pcall(function()
		return SearchIndexAPI.startEditSession(args.photoId, originalPath, fields)
	end)
	removeFile(originalPath)
	if not okCall then
		result.error = tostring(ok)
		return result
	end
	if not ok then
		result.error = Util.userFacingError(tostring(step))
		return result
	end

	local sessionId = step.session_id
	local tables = {}
	local currentRound = nil
	-- What Lightroom made of each round's masks. The model hears it with the
	-- render; the user hears it for the round the photo ends on.
	local roundNotes = {}

	local function fail(message)
		if currentRound ~= nil then
			table.insert(result.warnings, message .. "; kept round " .. tostring(currentRound))
			result.keptRound = currentRound
		else
			result.error = message
		end
	end

	while true do
		appendAll(result.warnings, step.warnings)
		appendAll(result.guardrailExplanations, step.guardrail_explanations)
		result.inputTokens = step.input_tokens
		result.outputTokens = step.output_tokens

		if step.done then
			local keep = tonumber(step.keep_round)
			result.verdict = step.verdict
			if keep == currentRound then
				result.recipe = step.edit
			end
			if keep ~= nil and keep ~= currentRound and tables[keep] then
				progress("going back to round " .. tostring(keep))
				local tbl = EditLoop.composeApply(baseline, tables[keep].table, result.state)
				local applied, err, backNotes = applyTable(
					photo,
					tbl,
					tables[keep].updateAI,
					"AI Edit: back to round " .. tostring(keep),
					tables[keep].table
				)
				if applied then
					currentRound = keep
					result.recipe = step.edit
					roundNotes[keep] = backNotes
				else
					table.insert(
						result.warnings,
						"could not go back to round "
							.. tostring(keep)
							.. " ("
							.. tostring(err)
							.. "); kept round "
							.. tostring(currentRound)
					)
				end
			end
			result.keptRound = currentRound
			break
		end

		if isCanceled() then
			result.canceled = true
			break
		end

		local round = tonumber(step.round) or 0
		table.insert(result.rounds, {
			round = round,
			source = step.source,
			assessment = step.assessment,
		})
		tables[round] = { table = step.develop_settings or {}, updateAI = step.update_ai_settings == true }
		progress(
			step.source == "style" and "applying your style"
				or (
					"applying round "
					.. tostring(round)
					.. " of up to "
					.. tostring(round + (tonumber(step.edits_left) or 0))
				)
		)
		local tbl = EditLoop.composeApply(baseline, tables[round].table, result.state)
		local historyName = step.source == "style" and "AI Edit: your style" or ("AI Edit: round " .. tostring(round))
		local applied, applyErr, notes =
			applyTable(photo, tbl, tables[round].updateAI, historyName, tables[round].table)
		if not applied then
			fail("could not apply round " .. tostring(round) .. ": " .. tostring(applyErr))
			break
		end
		currentRound = round
		result.applied = true
		result.recipe = step.edit
		roundNotes[round] = notes

		if isCanceled() then
			result.canceled = true
			break
		end

		progress("rendering round " .. tostring(round))
		local renderPath, err = render(photo)
		if not renderPath then
			fail("could not render round " .. tostring(round) .. ": " .. tostring(err))
			break
		end
		progress("the AI model is looking at round " .. tostring(round))
		local okSend, sent, nextStep = LrTasks.pcall(function()
			return SearchIndexAPI.sendEditRender(sessionId, renderPath, notes)
		end)
		removeFile(renderPath)
		if not okSend then
			fail("the AI model's answer was lost: " .. tostring(sent))
			break
		end
		if not sent then
			fail("the AI model did not answer: " .. Util.userFacingError(tostring(nextStep)))
			break
		end
		step = nextStep
	end

	-- A mask of the round the photo ends on that found nothing, or was not
	-- computed in time, is on the photo doing nothing: the user has to know.
	if result.keptRound ~= nil and not result.canceled then
		appendAll(result.warnings, roundNotes[result.keptRound])
	end

	-- A session that finished ended itself on the backend; any other one is
	-- ended here so it does not wait out its expiry. Not reported when that
	-- fails: the backend drops an idle session after 30 minutes anyway, and
	-- nothing on the photo depends on it.
	if not step.done then
		local okEnd, ended, endErr = LrTasks.pcall(function()
			return SearchIndexAPI.endEditSession(sessionId)
		end)
		if not okEnd or not ended then
			log:warn("EditLoop: could not end session: " .. tostring(okEnd and endErr or ended))
		end
	end
	return result
end

---
-- Takes the photo back to where the loop found it.
-- @param photo LrPhoto
-- @param result table From EditLoop.run.
-- @return boolean ok, string|nil error
---
function EditLoop.revert(photo, result)
	if not result or not result.baseline then
		return false, "nothing to go back to"
	end
	local tbl = EditLoop.composeRevert(result.baseline, result.state)
	if next(tbl) == nil then
		return true, nil
	end
	local ok, err = applyTable(photo, tbl, false, "AI Edit: discarded", nil)
	return ok, err
end
