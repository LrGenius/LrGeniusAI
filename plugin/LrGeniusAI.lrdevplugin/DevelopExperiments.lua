--- Pure helpers for `TaskDevelopExperiments.lua`.
--
-- The task runs controlled experiments inside Lightroom to settle the open
-- questions from the AI Edit XMP research (docs/wiki/Dev-AI-Edit-XMP-Findings.md):
-- which develop keys `applyDevelopSettings` accepts, whether it can add AI masks,
-- how plugin presets behave, and in which frame the SDK reports dimensions.
--
-- Everything in here is free of Lightroom calls so it can be unit tested with
-- busted: building the develop-settings tables the experiments apply, reading
-- the results back into plain data, and rendering the report.

DevelopExperiments = {}

local HEX_DIGITS = "0123456789ABCDEF"

--- Generates a 32-digit upper-case hex id, the format of `CorrectionSyncID`
-- and `MaskSyncID`.
--
-- The ids must differ between runs: Lightroom may match a preset correction to
-- an existing one by its sync id, and a repeated id would turn "add a mask"
-- into "update the mask from the previous run".
--
-- @param random function|nil `random(1, 16)` source, `math.random` by default.
-- @return string
function DevelopExperiments.newSyncId(random)
	random = random or math.random
	local digits = {}
	for i = 1, 32 do
		local n = random(1, 16)
		digits[i] = HEX_DIGITS:sub(n, n)
	end
	return table.concat(digits)
end

--- `LocalExposure2012` is stored as EV/4, so +1 EV is 0.25. Every other signed
-- local slider is stored as UI value / 100.
function DevelopExperiments.localExposureFromStops(stops)
	return stops / 4
end

-- The local keys Adobe's own Adaptive presets write for every correction. The
-- experiment corrections mirror that shape exactly, so a rejected correction
-- cannot be blamed on a key the proven form would have carried.
local LOCAL_KEYS = {
	"LocalExposure",
	"LocalHue",
	"LocalSaturation",
	"LocalContrast",
	"LocalClarity",
	"LocalSharpness",
	"LocalBrightness",
	"LocalToningHue",
	"LocalToningSaturation",
	"LocalExposure2012",
	"LocalContrast2012",
	"LocalHighlights2012",
	"LocalShadows2012",
	"LocalWhites2012",
	"LocalBlacks2012",
	"LocalClarity2012",
	"LocalDehaze",
	"LocalLuminanceNoise",
	"LocalMoire",
	"LocalDefringe",
	"LocalTemperature",
	"LocalTint",
	"LocalTexture",
}

--- The AI mask kinds the experiments exercise, in the "compute me" form
-- Adobe's Adaptive presets use: no digests, no geometry, only what to select.
DevelopExperiments.MASKS = {
	subject = { subType = 1, maskName = "Subject 1" },
	sky = { subType = 2, maskName = "Sky 1" },
	background = { subType = 0, subCategoryId = 22, maskName = "Background 1" },
	-- People part in preset form: 3 reportedly means "every person"
	-- (unconfirmed), 5 = hair.
	hair = { subType = 3, subCategoryId = 5, maskName = "Hair" },
}

--- Builds one AI mask tool (a `CorrectionMasks` entry).
-- @param mask table One of `DevelopExperiments.MASKS`.
-- @param syncId string The tool's `MaskSyncID`.
function DevelopExperiments.aiMaskTool(mask, syncId)
	local tool = {
		What = "Mask/Image",
		MaskActive = true,
		MaskName = mask.maskName,
		MaskBlendMode = 0,
		MaskInverted = false,
		MaskSyncID = syncId,
		MaskValue = 1,
		MaskVersion = 1,
		MaskSubType = mask.subType,
		ReferencePoint = "0.500000 0.500000",
		ErrorReason = 0,
	}
	if mask.subCategoryId ~= nil then
		tool.MaskSubCategoryID = mask.subCategoryId
	end
	return tool
end

--- Builds a `MaskGroupBasedCorrections` entry holding one AI mask.
--
-- Runtime ids (`CorrectionID`, `MaskID`) and computed fields (`MaskDigest`,
-- `InputDigest`, `FullMaskSize`, ...) are deliberately absent: they are what
-- Lightroom adds when it computes the mask, and their appearance is what the
-- experiments look for.
--
-- @param spec table `{ name, mask, exposureStops, syncId?, maskSyncId? }`.
-- @param newId function|nil Id source for missing sync ids.
function DevelopExperiments.aiMaskCorrection(spec, newId)
	newId = newId or DevelopExperiments.newSyncId
	local correction = {
		What = "Correction",
		CorrectionAmount = 1,
		CorrectionActive = true,
		CorrectionName = spec.name,
		CorrectionSyncID = spec.syncId or newId(),
	}
	for _, key in ipairs(LOCAL_KEYS) do
		correction[key] = 0
	end
	correction.LocalExposure2012 = DevelopExperiments.localExposureFromStops(spec.exposureStops or 1)
	correction.CorrectionMasks = {
		DevelopExperiments.aiMaskTool(spec.mask, spec.maskSyncId or newId()),
	}
	return correction
end

--- Builds a `MaskGroupBasedCorrections` entry holding one linear gradient.
--
-- Gradients need no AI computation, and third-party plugins already write them
-- through `applyDevelopSettings`, so they make a dependable "existing mask" for
-- experiments that ask what happens to masks a photo already has.
--
-- @param spec table `{ name, exposureStops, zeroX, zeroY, fullX, fullY }`,
--   coordinates normalised to the uncropped sensor-oriented image.
-- @param newId function|nil Id source for the sync ids.
function DevelopExperiments.gradientCorrection(spec, newId)
	newId = newId or DevelopExperiments.newSyncId
	local correction = {
		What = "Correction",
		CorrectionAmount = 1,
		CorrectionActive = true,
		CorrectionName = spec.name,
		CorrectionSyncID = spec.syncId or newId(),
	}
	for _, key in ipairs(LOCAL_KEYS) do
		correction[key] = 0
	end
	correction.LocalExposure2012 = DevelopExperiments.localExposureFromStops(spec.exposureStops or -1)
	correction.CorrectionMasks = {
		{
			What = "Mask/Gradient",
			MaskActive = true,
			MaskName = "Linear Gradient 1",
			MaskBlendMode = 0,
			MaskInverted = false,
			MaskSyncID = newId(),
			MaskValue = 1,
			ZeroX = spec.zeroX or 0.5,
			ZeroY = spec.zeroY or 0.6,
			FullX = spec.fullX or 0.5,
			FullY = spec.fullY or 0.2,
		},
	}
	return correction
end

--- Returns a new array: the existing corrections, unchanged, followed by the
-- additions. `applyDevelopSettings` replaces a top-level key wholesale, so the
-- array handed to it has to carry every correction the photo should keep.
function DevelopExperiments.appendCorrections(existing, additions)
	local out = {}
	if type(existing) == "table" then
		for _, correction in ipairs(existing) do
			table.insert(out, correction)
		end
	end
	for _, correction in ipairs(additions or {}) do
		table.insert(out, correction)
	end
	return out
end

--- Finds every correction carrying the given `CorrectionSyncID`.
-- @return table Array of matching corrections (empty when there is none).
function DevelopExperiments.findCorrections(groups, syncId)
	local found = {}
	if type(groups) ~= "table" then
		return found
	end
	for _, correction in ipairs(groups) do
		if type(correction) == "table" and correction.CorrectionSyncID == syncId then
			table.insert(found, correction)
		end
	end
	return found
end

local function isNonZero(value)
	if value == nil then
		return false
	end
	local number = tonumber(value)
	if number == nil then
		return true
	end
	return number ~= 0
end

--- Classifies one mask tool:
--   * "computed" - Lightroom attached a mask bitmap (`MaskDigest`)
--   * "failed"   - computed, but nothing was found (`ErrorReason` 1)
--   * "pending"  - still only the definition we wrote
--   * "geometric" - not an AI mask at all
function DevelopExperiments.toolState(tool)
	if type(tool) ~= "table" or tool.What ~= "Mask/Image" then
		return "geometric"
	end
	if tool.MaskDigest ~= nil and tool.MaskDigest ~= "" then
		return "computed"
	end
	if isNonZero(tool.ErrorReason) then
		return "failed"
	end
	return "pending"
end

--- Aggregates the states of a correction's AI tools. Returns "missing" for a
-- nil correction so a poll loop can tell "not there" from "not computed yet".
function DevelopExperiments.correctionState(correction)
	if type(correction) ~= "table" then
		return "missing"
	end
	local sawPending, sawFailed, sawComputed = false, false, false
	for _, tool in ipairs(correction.CorrectionMasks or {}) do
		local state = DevelopExperiments.toolState(tool)
		if state == "pending" then
			sawPending = true
		elseif state == "failed" then
			sawFailed = true
		elseif state == "computed" then
			sawComputed = true
		end
	end
	if sawPending then
		return "pending"
	end
	if sawFailed then
		return "failed"
	end
	if sawComputed then
		return "computed"
	end
	return "no-ai-mask"
end

--- The first non-zero `ErrorReason` among the tools of summarised corrections.
local function firstErrorReason(summary)
	for _, correction in ipairs(summary or {}) do
		for _, tool in ipairs(correction.tools or {}) do
			if isNonZero(tool.errorReason) then
				return tool.errorReason
			end
		end
	end
	return nil
end

--- Renders a poll result for a verdict line. "failed" is how Lightroom marks a
-- mask it computed and found nothing for (no person, no sky): the definition
-- was accepted, so the wording must not suggest it was rejected.
-- @param poll table `{ state, seconds, timedOut, summary }` as `pollCorrection` builds it.
function DevelopExperiments.describePoll(poll)
	if type(poll) ~= "table" then
		return "not polled"
	end
	local seconds = tostring(poll.seconds)
	if poll.state == "unreadable" then
		return "unknown - the develop settings could not be read back after " .. seconds .. " s"
	end
	if poll.state == "canceled" then
		return "not finished (run canceled after " .. seconds .. " s)"
	end
	if poll.state == "failed" then
		return string.format(
			"computed, nothing found (ErrorReason=%s) after %s s",
			tostring(firstErrorReason(poll.summary) or "?"),
			seconds
		)
	end
	if poll.timedOut then
		return "still " .. tostring(poll.state) .. " after " .. seconds .. " s"
	end
	return tostring(poll.state) .. " after " .. seconds .. " s"
end

--- Reduces `MaskGroupBasedCorrections` to the fields the report needs.
function DevelopExperiments.summarizeCorrections(groups)
	local summary = {}
	if type(groups) ~= "table" then
		return summary
	end
	for _, correction in ipairs(groups) do
		if type(correction) == "table" then
			local tools = {}
			for _, tool in ipairs(correction.CorrectionMasks or {}) do
				table.insert(tools, {
					what = tool.What,
					name = tool.MaskName,
					subType = tool.MaskSubType,
					subCategoryId = tool.MaskSubCategoryID,
					state = DevelopExperiments.toolState(tool),
					errorReason = tool.ErrorReason,
					fullMaskSize = tool.FullMaskSize,
					referencePoint = tool.ReferencePoint,
					hasMaskId = tool.MaskID ~= nil,
				})
			end
			table.insert(summary, {
				name = correction.CorrectionName,
				syncId = correction.CorrectionSyncID,
				amount = correction.CorrectionAmount,
				localExposure2012 = correction.LocalExposure2012,
				hasCorrectionId = correction.CorrectionID ~= nil,
				state = DevelopExperiments.correctionState(correction),
				tools = tools,
			})
		end
	end
	return summary
end

--- Copies the listed keys out of a settings table. Missing keys stay missing,
-- so "Lightroom dropped the key" and "the key is 0" remain distinguishable.
function DevelopExperiments.pick(settings, keys)
	local out = {}
	if type(settings) ~= "table" then
		return out
	end
	for _, key in ipairs(keys) do
		if settings[key] ~= nil then
			out[key] = settings[key]
		end
	end
	return out
end

--- Lists the keys whose values differ between two picked subsets, as
-- `{ key = { before = ..., after = ... } }`. A key present on only one side
-- counts as changed.
function DevelopExperiments.diff(before, after, keys)
	local changes = {}
	for _, key in ipairs(keys) do
		local a = before and before[key]
		local b = after and after[key]
		if a ~= b then
			changes[key] = { before = a, after = b }
		end
	end
	return changes
end

--- Parses Lightroom's dimensions in either form the SDK uses: the raw
-- `{ width = 2304, height = 3072 }` table or the formatted "3072 x 2304".
-- @return number|nil, number|nil width, height
function DevelopExperiments.parseDimensions(value)
	if type(value) == "table" then
		local w, h = tonumber(value.width), tonumber(value.height)
		if w and h then
			return w, h
		end
		return nil, nil
	end
	if type(value) == "string" then
		local w, h = value:match("(%d+)%s*[xX×]%s*(%d+)")
		if w and h then
			return tonumber(w), tonumber(h)
		end
	end
	return nil, nil
end

--- Predicts the cropped size from the crop settings and compares it with what
-- Lightroom reports, under every combination of "which way round" the two
-- dimension values are.
--
-- The model is the one the XMP corpus established (570/570 angled crops):
-- CropLeft/Top and CropRight/Bottom are opposite corners of a rectangle rotated
-- by +CropAngle, normalised to the *sensor-oriented* uncropped image. Here it
-- answers which orientation `getRawMetadata('dimensions')` and
-- `'croppedDimensions'` use at runtime; the corpus only proved it for the
-- catalog columns.
--
-- @param dims table `{ w, h }` as reported for the whole image.
-- @param cropped table `{ w, h }` as reported for the cropped image.
-- @param crop table `{ left, top, right, bottom, angle }` from the develop settings.
-- @return table Array of `{ dimensions, cropped, predictedW, predictedH, errorPx }`,
--   best (smallest error) first.
function DevelopExperiments.cropModelCheck(dims, cropped, crop)
	local left = tonumber(crop.left) or 0
	local top = tonumber(crop.top) or 0
	local right = tonumber(crop.right) or 1
	local bottom = tonumber(crop.bottom) or 1
	local theta = math.rad(tonumber(crop.angle) or 0)

	local results = {}
	local dimsVariants = {
		{ label = "as reported", w = dims.w, h = dims.h },
		{ label = "swapped", w = dims.h, h = dims.w },
	}
	local croppedVariants = {
		{ label = "as reported", w = cropped.w, h = cropped.h },
		{ label = "swapped", w = cropped.h, h = cropped.w },
	}
	for _, d in ipairs(dimsVariants) do
		local dx = (right - left) * d.w
		local dy = (bottom - top) * d.h
		local predictedW = math.cos(theta) * dx + math.sin(theta) * dy
		local predictedH = -math.sin(theta) * dx + math.cos(theta) * dy
		for _, c in ipairs(croppedVariants) do
			local errorPx = math.max(math.abs(predictedW - c.w), math.abs(predictedH - c.h))
			table.insert(results, {
				dimensions = d.label,
				cropped = c.label,
				predictedW = predictedW,
				predictedH = predictedH,
				errorPx = errorPx,
			})
		end
	end
	-- Deterministic on ties: Lua 5.1's sort is not stable, and a tie is exactly
	-- the case `describeCropModel` has to recognise instead of picking one.
	table.sort(results, function(a, b)
		if a.errorPx ~= b.errorPx then
			return a.errorPx < b.errorPx
		end
		return a.dimensions .. "/" .. a.cropped < b.dimensions .. "/" .. b.cropped
	end)
	return results
end

-- Lightroom's orientation codes, as the catalog stores them, mapped to EXIF
-- orientation numbers. Only AB/BC/CD/DA/CB have been seen in real catalogs;
-- the mirrored ones are the natural completion and are unverified.
local ORIENTATION_TO_EXIF = {
	AB = 1,
	BA = 2,
	CD = 3,
	DC = 4,
	CB = 5,
	BC = 6,
	AD = 7,
	DA = 8,
}

--- @return number|nil The EXIF orientation for a Lightroom orientation code.
function DevelopExperiments.exifOrientation(code)
	if type(code) == "number" then
		return code
	end
	if type(code) ~= "string" then
		return nil
	end
	return ORIENTATION_TO_EXIF[code:upper()]
end

--- True when the orientation turns the image by 90° (EXIF 5-8), i.e. when
-- sensor and display width/height are swapped.
function DevelopExperiments.isQuarterTurn(code)
	local exif = DevelopExperiments.exifOrientation(code)
	return exif ~= nil and exif >= 5 and exif <= 8
end

--- Interprets `cropModelCheck` for the report. Only a quarter-turned photo can
-- tell sensor from display orientation, and only when the best hypothesis
-- matches to within a few pixels.
-- @return string
function DevelopExperiments.describeCropModel(results, orientationCode, tolerancePx)
	tolerancePx = tolerancePx or 3
	local best = results and results[1]
	if not best then
		return "no crop model result"
	end
	if best.errorPx > tolerancePx then
		return string.format(
			"no hypothesis matches (best error %.1f px) - the crop model or the reported sizes are not what we think",
			best.errorPx
		)
	end
	if not DevelopExperiments.isQuarterTurn(orientationCode) then
		return string.format(
			"crop model matches (error %.1f px); orientation %s cannot tell sensor from display frame",
			best.errorPx,
			tostring(orientationCode)
		)
	end
	-- Decide each frame only from the hypotheses that fit. With no crop, or an
	-- unrotated crop that keeps the image's aspect ratio, "both as reported"
	-- and "both swapped" fit equally well, and picking one would state a frame
	-- the data does not show.
	local dimsLabels, croppedLabels = {}, {}
	for _, result in ipairs(results) do
		if result.errorPx <= tolerancePx then
			dimsLabels[result.dimensions] = true
			croppedLabels[result.cropped] = true
		end
	end
	local function frameOf(labels)
		if labels["as reported"] and labels["swapped"] then
			return nil
		end
		return labels["as reported"] and "sensor" or "display"
	end
	local dimsFrame, croppedFrame = frameOf(dimsLabels), frameOf(croppedLabels)
	if not dimsFrame or not croppedFrame then
		return string.format(
			"ambiguous (error %.1f px): no crop, or an unrotated crop that keeps the aspect ratio, fits sensor and display frames equally - straighten the crop or change its aspect ratio",
			best.errorPx
		)
	end
	return string.format(
		"dimensions are reported in %s orientation, croppedDimensions in %s orientation (error %.1f px)",
		dimsFrame,
		croppedFrame,
		best.errorPx
	)
end

--- True when the running Lightroom is at least `major.minor`.
function DevelopExperiments.versionAtLeast(versionTable, major, minor)
	if type(versionTable) ~= "table" then
		return false
	end
	local vMajor = tonumber(versionTable.major) or 0
	local vMinor = tonumber(versionTable.minor) or 0
	if vMajor ~= major then
		return vMajor > major
	end
	return vMinor >= (minor or 0)
end

local function isArray(t)
	local count = 0
	for _ in pairs(t) do
		count = count + 1
	end
	for i = 1, count do
		if t[i] == nil then
			return false
		end
	end
	return true
end

--- Converts any value into plain data the JSON encoder and the Markdown
-- renderer can handle: tables become arrays or string-keyed maps, anything
-- else that is not a string, number or boolean becomes its `tostring`.
function DevelopExperiments.plainValue(value, depth)
	depth = depth or 0
	local kind = type(value)
	if kind == "string" or kind == "number" or kind == "boolean" or kind == "nil" then
		return value
	end
	if kind ~= "table" then
		return tostring(value)
	end
	if depth >= 8 then
		return "<nested table>"
	end
	local out = {}
	if isArray(value) then
		for i, item in ipairs(value) do
			out[i] = DevelopExperiments.plainValue(item, depth + 1)
		end
		return out
	end
	for key, item in pairs(value) do
		out[tostring(key)] = DevelopExperiments.plainValue(item, depth + 1)
	end
	return out
end

--- The keys of a table, sorted by their string form.
function DevelopExperiments.sortedKeys(t)
	local keys = {}
	for key in pairs(t or {}) do
		table.insert(keys, key)
	end
	table.sort(keys, function(a, b)
		return tostring(a) < tostring(b)
	end)
	return keys
end

local sortedKeys = DevelopExperiments.sortedKeys

--- Compact, deterministic one-line rendering of plain data for the Markdown
-- report (the JSON file carries the same data in full).
function DevelopExperiments.inlineValue(value, maxLength)
	maxLength = maxLength or 2000
	local function render(v)
		local kind = type(v)
		if kind == "string" then
			return string.format("%q", v)
		end
		if kind ~= "table" then
			return tostring(v)
		end
		local parts = {}
		if isArray(v) then
			for _, item in ipairs(v) do
				table.insert(parts, render(item))
			end
			return "[" .. table.concat(parts, ", ") .. "]"
		end
		for _, key in ipairs(sortedKeys(v)) do
			table.insert(parts, tostring(key) .. "=" .. render(v[key]))
		end
		return "{" .. table.concat(parts, ", ") .. "}"
	end
	local text = render(value)
	if #text > maxLength then
		text = text:sub(1, maxLength) .. " ... (truncated, see JSON)"
	end
	return text
end

local function isStringList(value)
	if type(value) ~= "table" or #value == 0 or not isArray(value) then
		return false
	end
	for _, item in ipairs(value) do
		if type(item) ~= "string" then
			return false
		end
	end
	return true
end

--- Renders the collected results as Markdown.
--
-- @param report table `{ meta = {...}, experiments = { { id, title, question,
--   verdicts = {}, manualChecks = {}, steps = { { photo, label, ok, error, data } } } } }`
-- @return string
function DevelopExperiments.renderMarkdown(report)
	local lines = {}
	local function add(line)
		table.insert(lines, line or "")
	end

	add("# LrGeniusAI develop-settings experiments")
	add()
	for _, key in ipairs(sortedKeys(report.meta or {})) do
		local value = report.meta[key]
		if isStringList(value) then
			-- Lists of names and paths (photos, preset files to delete) are
			-- rendered verbatim: quoting doubles Windows backslashes, and
			-- truncation would cut the very paths the user has to act on.
			add(string.format("- **%s:**", tostring(key)))
			for _, item in ipairs(value) do
				add("  - " .. item)
			end
		else
			add(string.format("- **%s:** %s", tostring(key), DevelopExperiments.inlineValue(value, 400)))
		end
	end

	for _, experiment in ipairs(report.experiments or {}) do
		add()
		add(string.format("## %s - %s", experiment.id, experiment.title))
		add()
		add("**Question:** " .. tostring(experiment.question))
		add()
		add("### Verdicts")
		add()
		if #(experiment.verdicts or {}) == 0 then
			add("- (none - see the steps below)")
		end
		for _, verdict in ipairs(experiment.verdicts or {}) do
			add("- " .. verdict)
		end
		if #(experiment.manualChecks or {}) > 0 then
			add()
			add("### Check by hand in Lightroom")
			add()
			for _, check in ipairs(experiment.manualChecks) do
				add("- [ ] " .. check)
			end
		end
		add()
		add("### Steps")
		for _, step in ipairs(experiment.steps or {}) do
			add()
			local status = step.ok and "ok" or "FAILED"
			add(string.format("#### %s - %s (%s)", tostring(step.photo), tostring(step.label), status))
			if step.error then
				add()
				add("Error: `" .. tostring(step.error) .. "`")
			end
			if step.data ~= nil then
				add()
				add("```")
				add(DevelopExperiments.inlineValue(step.data))
				add("```")
			end
		end
	end
	add()
	return table.concat(lines, "\n")
end

return DevelopExperiments
