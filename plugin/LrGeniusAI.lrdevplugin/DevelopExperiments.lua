--- Pure helpers for `TaskDevelopExperiments.lua`.
--
-- The task runs controlled experiments inside Lightroom to settle the open
-- questions from the AI Edit XMP research (docs/wiki/Dev-AI-Edit-XMP-Findings.md):
-- which develop keys `applyDevelopSettings` accepts, whether it can add AI masks
-- and transfer a profile `Look`, how plugin presets behave, in which frame the
-- SDK reports dimensions, and what a photo's full develop-settings table holds.
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
	if poll.state == "no-ai-mask" then
		return "no longer an AI mask - the correction's Mask/Image tool is gone after " .. seconds .. " s"
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

---------------------------------------------------------------------------
-- E1: white balance mode without values
---------------------------------------------------------------------------

-- The two white-balance key families. Which one a photo uses decides where a
-- recomputed value has to show up.
local WB_FAMILY_KEYS = {
	raw = { temperature = "Temperature", tint = "Tint" },
	["non-raw"] = { temperature = "IncrementalTemperature", tint = "IncrementalTint" },
}

--- The white-balance family of a develop-settings table: "raw" when it holds
-- `Temperature`, "non-raw" when it holds `IncrementalTemperature`, nil when it
-- holds neither. Unlike the file format this is right for a DNG converted from
-- a JPEG, which Lightroom treats as non-raw.
function DevelopExperiments.settingsFamily(settings)
	if type(settings) ~= "table" then
		return nil
	end
	if settings.Temperature ~= nil then
		return "raw"
	end
	if settings.IncrementalTemperature ~= nil then
		return "non-raw"
	end
	return nil
end

-- A Custom white balance no camera or Auto result is likely to land on. A
-- mode-only variant first puts its copy on this, in a history step of its own,
-- so any recomputation shows as a change, and "already on this mode" or "the
-- new mode happens to give the same values" cannot happen.
local WB_PRECONDITIONS = {
	raw = { WhiteBalance = "Custom", Temperature = 3000, Tint = 40 },
	["non-raw"] = { WhiteBalance = "Custom", IncrementalTemperature = -40, IncrementalTint = 40 },
}

--- The precondition a mode-only variant writes first, for a white-balance
-- family ("raw" or "non-raw"). A fresh table each call.
function DevelopExperiments.wbPrecondition(family)
	local out = {}
	for key, value in pairs(WB_PRECONDITIONS[family] or WB_PRECONDITIONS.raw) do
		out[key] = value
	end
	return out
end

--- True when the readback shows the precondition as written.
function DevelopExperiments.preconditionHeld(precondition, readback)
	if type(precondition) ~= "table" or type(readback) ~= "table" then
		return false
	end
	for key, value in pairs(precondition) do
		if readback[key] ~= value then
			return false
		end
	end
	return true
end

--- True when the family's temperature or tint differs between two readbacks.
function DevelopExperiments.wbPairChanged(family, before, after)
	local keys = WB_FAMILY_KEYS[family] or WB_FAMILY_KEYS.raw
	before, after = before or {}, after or {}
	return before[keys.temperature] ~= after[keys.temperature] or before[keys.tint] ~= after[keys.tint]
end

--- Describes what writing only a white-balance mode (no Temperature/Tint) did.
--
-- @param mode string The `WhiteBalance` value that was written.
-- @param family string "raw" or "non-raw": which key pair to look at.
-- @param before table White-balance keys of the copy before the write.
-- @param after table The same keys at the end of the wait.
-- @param opts table|nil `{ immediate, settle, flatten }`: `immediate` is the
--   readback right after the call; `settle` is `{ seconds, changed, canceled }`
--   from the wait for a change of the pair (`seconds` is when it changed, or
--   how long nothing did); `flatten` says optFlattenAutoNow was passed.
-- @return string
function DevelopExperiments.describeWbModeOutcome(mode, family, before, after, opts)
	before = before or {}
	after = after or {}
	opts = opts or {}
	local immediate, settle = opts.immediate, opts.settle
	local keys = WB_FAMILY_KEYS[family] or WB_FAMILY_KEYS.raw
	local pair = { keys.temperature, keys.tint }

	local changes = {}
	for _, key in ipairs(pair) do
		if before[key] ~= after[key] then
			table.insert(changes, string.format("%s %s -> %s", key, tostring(before[key]), tostring(after[key])))
		end
	end

	local parts = {}
	local flattened = opts.flatten and after.WhiteBalance ~= mode and #changes > 0
	if after.WhiteBalance == mode then
		table.insert(parts, "WhiteBalance = " .. tostring(mode))
	elseif flattened then
		-- Resolving Auto synchronously is what the flag asks for; a mode that
		-- reads Custom afterwards is that resolution, not a rejection.
		table.insert(
			parts,
			string.format("%s was flattened to WhiteBalance = %s", tostring(mode), tostring(after.WhiteBalance))
		)
	else
		table.insert(parts, "WhiteBalance was not taken (reads " .. tostring(after.WhiteBalance) .. ")")
	end

	local values
	if #changes > 0 then
		values = (flattened and "with " or "Lightroom recomputed ") .. table.concat(changes, ", ")
		if type(settle) == "table" and settle.changed then
			if (tonumber(settle.seconds) or 0) <= 0 then
				values = values .. " (already in the readback right after the call)"
			else
				values = values .. " after " .. tostring(settle.seconds) .. " s"
			end
		end
	else
		local within = ""
		if type(settle) == "table" then
			within = " within " .. tostring(settle.seconds) .. " s"
			if settle.canceled then
				within = within .. " (run canceled)"
			end
		end
		values = string.format(
			"%s/%s not recomputed%s (still %s / %s)",
			keys.temperature,
			keys.tint,
			within,
			tostring(before[keys.temperature]),
			tostring(before[keys.tint])
		)
	end
	if before.WhiteBalance == mode then
		values = values .. " - the copy was already on " .. tostring(mode) .. ", so this proves nothing"
	end
	table.insert(parts, values)

	if type(immediate) == "table" and next(DevelopExperiments.diff(immediate, after, pair)) ~= nil then
		table.insert(
			parts,
			string.format(
				"the readback right after the call still showed %s = %s, so Lightroom resolves the mode asynchronously",
				keys.temperature,
				tostring(immediate[keys.temperature])
			)
		)
	end

	-- The other family appearing would mean Lightroom changed its mind about
	-- what kind of file this is; worth a line of its own.
	local other = family == "non-raw" and WB_FAMILY_KEYS.raw or WB_FAMILY_KEYS["non-raw"]
	for _, key in ipairs({ other.temperature, other.tint }) do
		if before[key] == nil and after[key] ~= nil then
			table.insert(parts, string.format("%s appeared (%s)", key, tostring(after[key])))
		end
	end
	return table.concat(parts, "; ")
end

---------------------------------------------------------------------------
-- E13: full develop-settings readback
---------------------------------------------------------------------------

-- Small all-scalar tables (tone curves, the LensBlur block) are copied with
-- their values: they are defaults worth having. Anything larger is described
-- by its shape only.
local MAX_INLINE_TABLE_ITEMS = 64

local function countKeys(t)
	local count = 0
	for _ in pairs(t) do
		count = count + 1
	end
	return count
end

local function isScalar(value)
	local kind = type(value)
	return kind == "string" or kind == "number" or kind == "boolean"
end

--- Describes a table by its shape: `{ type = "table", shape = "array" |
-- "map" | "empty", length, keyCount, values? }`. `values` is present only for
-- small tables whose entries are all scalars.
function DevelopExperiments.describeTable(t)
	local keyCount = countKeys(t)
	local shape = keyCount == 0 and "empty" or (isArray(t) and "array" or "map")
	local out = { type = "table", shape = shape, length = #t, keyCount = keyCount }
	if keyCount > 0 and keyCount <= MAX_INLINE_TABLE_ITEMS then
		local allScalar = true
		for _, value in pairs(t) do
			if not isScalar(value) then
				allScalar = false
				break
			end
		end
		if allScalar then
			out.values = DevelopExperiments.plainValue(t)
		end
	end
	return out
end

--- Summarises a `Look` without copying its `Parameters` (Adobe's profile
-- definition): name, UUID, amount, whether `Parameters` exist and how many
-- keys they have, and the Look's own key list.
-- @return table|nil nil when `look` is not a table.
function DevelopExperiments.summarizeLook(look)
	if type(look) ~= "table" then
		return nil
	end
	local params = look.Parameters
	local keys = {}
	for _, key in ipairs(sortedKeys(look)) do
		table.insert(keys, tostring(key))
	end
	return {
		Name = look.Name,
		UUID = look.UUID,
		Amount = look.Amount,
		Stubbed = look.Stubbed,
		isAdobeAdaptive = look.isAdobeAdaptive,
		CameraModelRestriction = look.CameraModelRestriction,
		hasParameters = type(params) == "table" and next(params) ~= nil,
		parameterKeyCount = type(params) == "table" and countKeys(params) or 0,
		keys = keys,
	}
end

--- Every top-level key of a `getDevelopSettings()` table: scalar values
-- verbatim, tables summarised (`describeTable`), `Look` via `summarizeLook`
-- and `MaskGroupBasedCorrections` via `summarizeCorrections`. Read from an
-- unedited photo this is Lightroom's default table for its file kind.
function DevelopExperiments.summarizeSettings(settings)
	local out = {}
	if type(settings) ~= "table" then
		return out
	end
	for key, value in pairs(settings) do
		local name = tostring(key)
		if isScalar(value) then
			out[name] = value
		elseif type(value) ~= "table" then
			out[name] = tostring(value)
		elseif name == "Look" then
			local summary = DevelopExperiments.summarizeLook(value)
			summary.type = "Look"
			out[name] = summary
		elseif name == "MaskGroupBasedCorrections" then
			out[name] = {
				type = "table",
				length = #value,
				corrections = DevelopExperiments.summarizeCorrections(value),
			}
		else
			out[name] = DevelopExperiments.describeTable(value)
		end
	end
	return out
end

---------------------------------------------------------------------------
-- E11: Look transfer
---------------------------------------------------------------------------

--- Adobe Raw profiles for the stubbed-Look variants (E11a, E11c), tried in
-- order; the first whose name differs from the photo's current Look is used,
-- so a result cannot be "unchanged" merely because it was already there.
DevelopExperiments.STUB_LOOKS = {
	{ Name = "Adobe Vivid", UUID = "EA1DE074F188405965EF399C72C221D9" },
	{ Name = "Adobe Landscape", UUID = "6F9C877E84273F4E8271E6B91BEB36A1" },
	{ Name = "Adobe Color", UUID = "B952C231111CD8E0ECCF14B86BAA7077" },
}

--- A stubbed Look without `Parameters`. By default in the form Lightroom's
-- own presets carry it, with `Stubbed = true` (every Look stub in the bundled
-- presets has it); `bare` leaves the flag out, so the difference between the
-- two forms is an answer of its own.
function DevelopExperiments.stubLook(candidate, bare)
	local look = { Name = candidate.Name, UUID = candidate.UUID, Amount = 1 }
	if not bare then
		look.Stubbed = true
	end
	return look
end

--- The base profile E11 writes under its Looks. Adobe Raw Looks sit on an
-- "Adobe Standard" profile; a master already on one ("Adobe Standard v2")
-- keeps it, so the Look is the only thing that changes.
function DevelopExperiments.e11CameraProfile(current)
	if type(current) == "string" and current:find("^Adobe Standard") then
		return current
	end
	return "Adobe Standard"
end

--- Why a Look cannot serve as E11b's full Look for a photo, or nil when it
-- can.
-- @param look table|nil The candidate Look.
-- @param currentName string|nil The photo's current `Look.Name`.
-- @param donorCamera string|nil Camera model of the photo the Look comes from.
-- @param targetCamera string|nil Camera model of the photo it would go to.
-- @return string|nil
function DevelopExperiments.lookRejection(look, currentName, donorCamera, targetCamera)
	if not DevelopExperiments.isFullLook(look) then
		return "no full Look (no Parameters)"
	end
	if look.Name == currentName then
		return "same Look as the photo (" .. tostring(currentName) .. ")"
	end
	if look.isAdobeAdaptive then
		return "an Adobe Adaptive Look, which also needs AILook and an AI update"
	end
	-- The restriction names Adobe's camera id, not the EXIF model ("Canon EOS
	-- R6 Mark II" vs "Canon EOS R6m2"), so it cannot be matched directly. A
	-- donor of the same camera model is known to satisfy it.
	if look.CameraModelRestriction ~= nil and (donorCamera == nil or donorCamera ~= targetCamera) then
		return string.format(
			"restricted to %s, and the photo is from %s",
			tostring(look.CameraModelRestriction),
			tostring(targetCamera)
		)
	end
	return nil
end

--- The first donor whose Look passes `lookRejection`.
-- @param donors table Array of `{ photo, camera, look }`.
-- @return table|nil, table the donor, and `{ { photo, reason } }` for every
--   donor passed over.
function DevelopExperiments.pickDonorLook(donors, currentName, targetCamera)
	local rejected = {}
	for _, donor in ipairs(donors or {}) do
		local reason = DevelopExperiments.lookRejection(donor.look, currentName, donor.camera, targetCamera)
		if reason == nil then
			return donor, rejected
		end
		table.insert(rejected, { photo = donor.photo, reason = reason })
	end
	return nil, rejected
end

--- The reason E11b's preset fallback found nothing, including presets whose
-- settings could not be read: a failed read must not pass for "no preset has
-- one".
-- @param scan table `{ scanned, failedReads, firstError, found }`.
-- @return string
function DevelopExperiments.describePresetScan(scan)
	scan = scan or {}
	local text = string.format(
		"none of the %d develop presets read carries a usable full Look (Lightroom's bundled presets store Looks as stubs)",
		tonumber(scan.scanned) or 0
	)
	if (tonumber(scan.failedReads) or 0) > 0 then
		text = text
			.. string.format(
				"; %d preset(s) could not be read, first error: %s",
				scan.failedReads,
				tostring(scan.firstError)
			)
	end
	return text
end

--- True for a Look that carries its full profile definition: a name and a
-- non-empty `Parameters` table.
function DevelopExperiments.isFullLook(look)
	return type(look) == "table"
		and type(look.Name) == "string"
		and look.Name ~= ""
		and type(look.Parameters) == "table"
		and next(look.Parameters) ~= nil
end

--- The first candidate whose Look name differs from `currentName`.
-- @param candidates table Array of candidates.
-- @param currentName string|nil The photo's current `Look.Name`.
-- @param nameOf function|nil Reads a candidate's name; `candidate.Name` by default.
-- @return table|nil
function DevelopExperiments.pickLook(candidates, currentName, nameOf)
	nameOf = nameOf or function(candidate)
		return candidate.Name
	end
	for _, candidate in ipairs(candidates or {}) do
		if nameOf(candidate) ~= currentName then
			return candidate
		end
	end
	return nil
end

--- Renders a Look state (`{ CameraProfile, look = summarizeLook(...) }`) for
-- a verdict line.
function DevelopExperiments.describeLookState(state)
	state = state or {}
	local look = state.look
	local lookText
	if look == nil then
		lookText = "no Look"
	else
		lookText = string.format(
			"Look %s (%s, Parameters: %s)",
			tostring(look.Name),
			tostring(look.UUID),
			look.hasParameters and "yes" or "no"
		)
	end
	return string.format("CameraProfile %s, %s", tostring(state.CameraProfile), lookText)
end

--- Judges one Look write: "honoured", "ignored", "changed to something else"
-- or "error".
--
-- @param applied table `{ CameraProfile, look = summarizeLook(written Look) }`.
-- @param before table Look state of the copy before the write.
-- @param after table Look state after the write.
-- @param ok boolean Whether the write gate ran without error.
-- @param err string|nil The error when it did not.
-- @param readErr string|nil A readback error, which makes the result unknown.
-- @return string
function DevelopExperiments.describeLookOutcome(applied, before, after, ok, err, readErr)
	if not ok then
		return "error: " .. tostring(err)
	end
	if readErr then
		return "applied, but the result is unknown - " .. tostring(readErr)
	end
	applied, before, after = applied or {}, before or {}, after or {}
	local wanted = applied.look or {}
	local got = after.look
	local was = before.look

	local parts = {}
	if got ~= nil and got.Name == wanted.Name and (wanted.UUID == nil or got.UUID == wanted.UUID) then
		table.insert(parts, "honoured")
		if wanted.hasParameters then
			table.insert(parts, got.hasParameters and "Parameters kept" or "Parameters dropped")
		elseif got.hasParameters then
			table.insert(parts, "Lightroom filled in the profile's Parameters")
		else
			table.insert(parts, "the Look still has no Parameters - check the profile browser")
		end
	elseif (got and got.Name) == (was and was.Name) and (got and got.UUID) == (was and was.UUID) then
		table.insert(parts, "ignored - the Look is unchanged")
	else
		table.insert(parts, "changed to something else")
	end
	if after.CameraProfile ~= applied.CameraProfile then
		table.insert(parts, "CameraProfile reads " .. tostring(after.CameraProfile))
	end
	table.insert(parts, "now " .. DevelopExperiments.describeLookState(after))
	return table.concat(parts, "; ")
end

---------------------------------------------------------------------------
-- E2: timing
---------------------------------------------------------------------------

local function seconds(value)
	if type(value) ~= "number" then
		return "?"
	end
	return string.format("%.1f", value)
end

-- "no-ai-mask" is deliberately absent: it means the correction lost its AI
-- tool, which is not a mask being ready and must not enter the runtimes.
local TERMINAL_MASK_STATES = { computed = true, failed = true }

--- Renders one photo's E2 timing:
--   * source "auto"   - Lightroom computed the mask without being asked
--   * source "update" - after an explicit `updateAISettings()`
--   * source "none"   - not measured, with `reason`
--
-- @param timing table `{ source, reason?, state?, updateOk?,
--   updateCallSeconds?, gateWaitSeconds?, updateSeconds?, readySeconds?,
--   waitedBeforeUpdate?, lockedDuringAutoWait? }`. `updateCallSeconds` is the
--   `updateAISettings()` call itself, `gateWaitSeconds` the wait for write
--   access before it, `updateSeconds` both together. `readySeconds` runs from
--   the start of the call (or the end of the apply, for "auto") to the final
--   mask state. `waitedBeforeUpdate` is how long the photo was locked before
--   the update could start, `lockedDuringAutoWait` whether it was ever locked
--   during the unasked wait; either means Lightroom was already computing, so
--   the time is only a lower bound.
-- @return string
function DevelopExperiments.describeTiming(timing)
	if type(timing) ~= "table" or timing.source == "none" or timing.source == nil then
		return "not measured (" .. tostring(type(timing) == "table" and timing.reason or "no data") .. ")"
	end
	local state = timing.state
	if timing.source == "auto" then
		if state == "failed" then
			return "without updateAISettings() the mask was computed (nothing found) after "
				.. seconds(timing.readySeconds)
				.. " s"
		end
		if state == "no-ai-mask" then
			return "without updateAISettings() the AI mask tool disappeared after "
				.. seconds(timing.readySeconds)
				.. " s"
		end
		return "without updateAISettings() the mask was ready after " .. seconds(timing.readySeconds) .. " s"
	end

	local callSeconds = timing.updateCallSeconds
	if type(callSeconds) ~= "number" then
		callSeconds = timing.updateSeconds
	end
	local text = "update call " .. seconds(callSeconds) .. " s"
	if type(timing.gateWaitSeconds) == "number" and timing.gateWaitSeconds >= 0.5 then
		text = text .. " (after " .. seconds(timing.gateWaitSeconds) .. " s waiting for write access)"
	end
	if not timing.updateOk then
		return text .. ", and it failed"
	end
	local outcome
	if state == "computed" then
		outcome = ", mask ready after " .. seconds(timing.readySeconds) .. " s"
	elseif state == "failed" then
		outcome = ", mask computed (nothing found) after " .. seconds(timing.readySeconds) .. " s"
	elseif state == "no-ai-mask" then
		return text .. ", the AI mask tool disappeared after " .. seconds(timing.readySeconds) .. " s"
	elseif state == "canceled" then
		return text .. ", run canceled " .. seconds(timing.readySeconds) .. " s after the update started"
	elseif state == "unreadable" then
		return text .. ", mask state unreadable"
	else
		outcome = ", mask still " .. tostring(state) .. " after " .. seconds(timing.readySeconds) .. " s"
	end
	local waited = tonumber(timing.waitedBeforeUpdate) or 0
	if waited > 0 or timing.lockedDuringAutoWait then
		local why = waited > 0
				and ("the photo was locked for " .. seconds(waited) .. " s before the update could start")
			or "the photo was locked during the unasked wait"
		outcome = outcome .. " - a lower bound: Lightroom was already computing before the update (" .. why .. ")"
	end
	return text .. outcome
end

--- True when a timing entry reached a final mask state that counts as a
-- mask runtime ("computed" or "failed" - not "no-ai-mask").
function DevelopExperiments.timingIsTerminal(timing)
	return type(timing) == "table" and TERMINAL_MASK_STATES[timing.state] == true
end

--- The run-level timing line: one `label: describeTiming(...)` per photo.
-- The first measured photo is marked: the first AI detection in a Lightroom
-- session also loads the model, and E2 is the run's first AI work.
-- @param timings table Array of timing entries, each with a `photo` label.
function DevelopExperiments.describeTimings(timings)
	local parts = {}
	local tagged = false
	for _, timing in ipairs(timings or {}) do
		local text = tostring(timing.photo) .. ": " .. DevelopExperiments.describeTiming(timing)
		if not tagged and type(timing) == "table" and timing.source ~= "none" and timing.source ~= nil then
			text = text .. " (includes loading the AI model if this was the first AI use this session)"
			tagged = true
		end
		table.insert(parts, text)
	end
	if #parts == 0 then
		return "E2 timing: no photo got as far as a mask"
	end
	return "E2 timing per photo - " .. table.concat(parts, "; ")
end

---------------------------------------------------------------------------
-- E4f: deleting a plugin preset file after applying it
---------------------------------------------------------------------------

local PRESET_FILE_EXTENSIONS = { xmp = true, lrtemplate = true }

--- Splits a path at its last "/" or "\".
-- @return string|nil, string the directory (nil when there is no separator)
--   and the leaf name.
function DevelopExperiments.splitPath(path)
	local dir, leaf = path:match("^(.*)[/\\]([^/\\]*)$")
	if dir == nil then
		return nil, path
	end
	return dir, leaf
end

local function isWindowsAbsolute(path)
	return path:match("^%a:[/\\]") ~= nil or path:match("^\\\\") ~= nil
end

--- The form paths are compared in: "/" separators, no trailing separator, and
-- lower case for Windows paths, whose file system ignores case.
local function comparablePath(path)
	local out = path:gsub("\\", "/"):gsub("/+$", "")
	if isWindowsAbsolute(path) then
		out = out:lower()
	end
	return out
end

--- Decides whether E4f may delete `path`, the file Lightroom reported for the
-- preset it just created. E4f deletes a file in a folder Lightroom shares with
-- every catalog, so the rule is narrow on purpose:
--   * an absolute path without "." or ".." components;
--   * a preset file (.xmp or .lrtemplate) whose name is `presetName`, alone
--     or followed by a suffix that starts with a non-alphanumeric character
--     ("E4f 2" matches "E4f", "E4fx" does not);
--   * not one of `knownFiles`, the preset files the other E4 steps created;
--   * in the same directory as one of `knownFiles`, or inside a folder named
--     "Plugin Develop Presets", where the SDK documents plugin presets live.
-- The caller still has to check that the path is a file, not a directory:
-- `LrFileUtils.delete` removes a directory with everything in it.
-- @return boolean, string|nil true, or false and why not.
function DevelopExperiments.presetFileDeletable(path, presetName, knownFiles)
	if type(path) ~= "string" or path == "" or path:find("^error:") then
		return false, "Lightroom reported no preset file path (" .. tostring(path) .. ")"
	end
	if type(presetName) ~= "string" or presetName == "" then
		return false, "there is no preset name to match the file against"
	end
	if path:sub(1, 1) ~= "/" and not isWindowsAbsolute(path) then
		return false, "the preset file path is not absolute: " .. path
	end
	local slashed = path:gsub("\\", "/")
	for component in slashed:gmatch("[^/]+") do
		if component == "." or component == ".." then
			return false, "the preset file path contains a relative component: " .. path
		end
	end

	local dir, leaf = DevelopExperiments.splitPath(path)
	local base, extension = leaf:match("^(.+)%.([^.]+)$")
	if base == nil or not PRESET_FILE_EXTENSIONS[extension:lower()] then
		return false, "the file is not a preset file (.xmp or .lrtemplate): " .. leaf
	end
	local rest = base:sub(#presetName + 1)
	if base:sub(1, #presetName) ~= presetName or (rest ~= "" and rest:match("^%w")) then
		return false, string.format("the file name %q does not match the preset name %q", leaf, presetName)
	end

	local target = comparablePath(path)
	local targetDir = comparablePath(dir)
	local nextToKnown = false
	for _, known in ipairs(knownFiles or {}) do
		if type(known) == "string" and known ~= "" then
			if comparablePath(known) == target then
				return false, "the path is the preset file of another experiment step: " .. path
			end
			local knownDir = DevelopExperiments.splitPath(known)
			if knownDir ~= nil and comparablePath(knownDir) == targetDir then
				nextToKnown = true
			end
		end
	end
	local inPluginFolder = false
	for component in dir:gsub("\\", "/"):gmatch("[^/]+") do
		if component:lower() == "plugin develop presets" then
			inPluginFolder = true
		end
	end
	if not nextToKnown and not inPluginFolder then
		return false,
			"the file is neither next to the other experiment preset files nor inside a 'Plugin Develop Presets' folder: "
				.. path
	end
	return true, nil
end

local function yesNo(value)
	return value and "yes" or "no"
end

--- Reads one numeric develop setting out of a preset file's text: the XMP
-- attribute (`crs:Contrast2012="+15"`), the XMP element
-- (`<crs:Contrast2012>+15</crs:Contrast2012>`) or the `.lrtemplate` field
-- (`Contrast2012 = 15,`). The key must stand on its own, so `Contrast2012`
-- does not match `crs:LocalContrast2012`.
-- @return number|nil nil when the key is not in the text.
function DevelopExperiments.presetFileSetting(content, key)
	if type(content) ~= "string" or type(key) ~= "string" or key == "" then
		return nil
	end
	local text = " " .. content
	local value = text:match("[:%s]" .. key .. '%s*=%s*"?%s*([%+%-]?[%d%.]+)')
		or text:match("<crs:" .. key .. ">%s*([%+%-]?[%d%.]+)%s*</crs:" .. key .. ">")
	return value and tonumber(value) or nil
end

--- Whether a mask state counts as computed: "computed", or "failed", which is
-- how Lightroom marks a mask it computed and found nothing for.
local function isComputedState(state)
	return state == "computed" or state == "failed"
end

local function maskWord(found)
	return found and "present" or "missing"
end

--- Joins paths for a verdict line, without repeats and at most three of them.
local function listPaths(paths)
	local unique, seen = {}, {}
	for _, path in ipairs(paths) do
		if not seen[path] then
			seen[path] = true
			table.insert(unique, path)
		end
	end
	local shown = {}
	for index = 1, math.min(#unique, 3) do
		table.insert(shown, unique[index])
	end
	local text = table.concat(shown, ", ")
	if #unique > 3 then
		text = text .. string.format(" and %d more", #unique - 3)
	end
	return text
end

local function describeDelete(d)
	if d.skipped then
		return "was not deleted - " .. tostring(d.skipped)
	end
	if not d.ok then
		return "could not be deleted - " .. tostring(d.error)
	end
	if d.existsAfter then
		return "is still there, although LrFileUtils.delete reported success"
	end
	return "was deleted"
end

local function describeSecondApply(s)
	if s.copyError then
		return "not tested (no second virtual copy: " .. tostring(s.copyError) .. ")"
	end
	if not s.ok then
		return "fails - " .. tostring(s.error)
	end
	if s.readError then
		return "no error, but the result is unknown - " .. tostring(s.readError)
	end
	local text
	if s.contrast == s.expectedContrast and s.maskFound then
		text = string.format("still works (Contrast2012 %s, subject mask present)", tostring(s.contrast))
	elseif s.contrast == s.expectedContrast then
		text = string.format("works partially (Contrast2012 yes (%s), subject mask missing)", tostring(s.contrast))
	else
		text = string.format(
			"has no effect (Contrast2012 %s, the preset holds %s; subject mask %s)",
			tostring(s.contrast),
			tostring(s.expectedContrast),
			maskWord(s.maskFound)
		)
	end
	if s.fileBack then
		text = text .. ", and it wrote the preset file again"
	end
	return text
end

--- What the presets of the E4f name listed before this run say. The three
-- cases answer different questions: a listed preset without its file is the
-- evidence that Lightroom keeps a deleted plugin preset somewhere else (when it
-- was restarted since the delete); one with its file is a leftover or a file
-- written back at quit; none at all after a restart means Lightroom forgot it.
local function describeBefore(r, add)
	local before = r.before
	if type(before) ~= "table" then
		return
	end
	local name = tostring(r.name)
	local missing, present, noPath = {}, {}, 0
	for _, entry in ipairs(before) do
		local file = type(entry) == "table" and entry.file or nil
		if type(file) ~= "string" or file == "" or file:find("^error:") then
			noPath = noPath + 1
		elseif entry.fileExists then
			table.insert(present, file)
		else
			table.insert(missing, file)
		end
	end
	if #missing + #present + noPath == 0 then
		add(
			string.format(
				"E4f no preset named %q was listed before this run. If an earlier E4f run deleted its file and Lightroom was restarted since, Lightroom forgot the deleted preset at the restart.",
				name
			)
		)
		return
	end
	if #missing > 0 then
		add(
			string.format(
				"E4f %d preset(s) named %q were listed before this run although no file exists at their path (%s). If an earlier E4f run deleted that file and Lightroom was restarted since, Lightroom keeps deleted plugin presets somewhere other than the file; without a restart in between it is only the list Lightroom holds in memory.",
				#missing,
				name,
				listPaths(missing)
			)
		)
	end
	if #present > 0 then
		add(
			string.format(
				"E4f %d preset(s) named %q were listed before this run with their file present (%s): left over from an earlier run that stopped before its delete, or written back by Lightroom at quit. This run writes and deletes a file of the same name, so that file may be gone afterwards; the paths are recorded here and in the report.",
				#present,
				name,
				listPaths(present)
			)
		)
	end
	if noPath > 0 then
		add(
			string.format(
				"E4f %d preset(s) named %q were listed before this run without a readable file path (preset:getFile() failed).",
				noPath,
				name
			)
		)
	end
end

--- The VIABLE / NOT viable line, judged on what actually arrived: the
-- contrast when it arrived, the subject mask when it arrived.
local function describeViability(r, a, first)
	local contrastArrived = a.contrast == r.expectedContrast
	local maskComputedBefore = isComputedState(a.maskPollState)
	local contrastKept = not contrastArrived or first.contrast == a.contrast
	-- A mask that was computed before the delete and is only a definition
	-- (or gone) after it lost its computation.
	local maskKept = not a.maskFound
		or (
			first.maskFound
			and not (maskComputedBefore and first.maskState ~= nil and not isComputedState(first.maskState))
		)
	local states = string.format(
		"mask state before the delete: %s, after: %s",
		tostring(a.maskState or "not polled"),
		tostring(first.maskState or "not read")
	)
	if contrastKept and maskKept then
		local text = string.format(
			"E4f apply then delete is VIABLE within this session (restart behaviour is a manual check): the preset file is gone and the first copy kept its edit (Contrast2012 %s, subject mask %s; %s).",
			tostring(first.contrast),
			maskWord(first.maskFound),
			states
		)
		if a.maskFound and not maskComputedBefore then
			text = text
				.. string.format(
					" The subject mask was still pending, not computed, when the file was deleted (%s), so the verdict covers the settings only; it does not show that a computed mask survives the delete.",
					tostring(a.maskState or "not polled")
				)
		end
		return text
	end
	return string.format(
		"E4f apply then delete is NOT viable: deleting the preset file changed the copy it was applied to (Contrast2012 %s -> %s, subject mask %s -> %s, %s).",
		tostring(a.contrast),
		tostring(first.contrast),
		maskWord(a.maskFound),
		maskWord(first.maskFound),
		states
	)
end

local function describeReadd(r, readd)
	local text
	if not readd.ok then
		return "E4f re-adding the same name after the delete failed - " .. tostring(readd.error) .. "."
	end
	local expected = readd.expectedContrast
	local fileText
	if readd.fileExists then
		text = string.format(
			"E4f re-adding the same name created a file again at %s (same path as the deleted file: %s; same uuid: %s)",
			tostring(readd.file),
			yesNo(readd.samePath),
			yesNo(readd.sameUuid)
		)
		if readd.fileContrast ~= nil then
			fileText = "the file holds " .. tostring(readd.fileContrast)
		else
			fileText = "no Contrast2012 found in the file"
		end
	else
		text = string.format(
			"E4f re-adding the same name returned a preset, but no file exists at its path %s (same uuid: %s)",
			tostring(readd.file),
			yesNo(readd.sameUuid)
		)
	end
	if expected ~= nil then
		local holdsNew = readd.contrast == expected and readd.fileHasNewContrast ~= false
		text = text
			.. string.format(
				"; holds the new settings (Contrast2012 %s, the deleted preset held %s): %s (getSetting reads %s%s)",
				tostring(expected),
				tostring(r.expectedContrast),
				yesNo(holdsNew),
				tostring(readd.contrast),
				fileText and (", " .. fileText) or ""
			)
	end
	if readd.namedCount ~= nil then
		text = text .. string.format("; presets of that name now listed: %s", tostring(readd.namedCount))
	end
	if readd.fileExists and type(readd.cleanup) == "table" then
		local cleanup = readd.cleanup
		local deletedAgain = not cleanup.skipped and cleanup.ok and not cleanup.existsAfter
		text = text .. "; the re-added file " .. describeDelete(cleanup)
		if not deletedAgain then
			text = text .. " and stays in the list of preset files to delete by hand"
		end
	end
	return text .. "."
end

--- Everything `describeE4f` says except the note on a canceled run.
local function describeE4fParts(r)
	local lines = {}
	local function add(line)
		table.insert(lines, line)
	end
	describeBefore(r, add)
	if type(r.create) ~= "table" then
		return lines
	end
	if not r.create.ok then
		add("E4f not run: the preset could not be created - " .. tostring(r.create.error))
		return lines
	end
	local a = r.apply
	if type(a) ~= "table" then
		return lines
	end
	if not a.ok then
		add("E4f not run: applying the preset to the first copy failed - " .. tostring(a.error))
		return lines
	end
	if a.readError then
		add("E4f inconclusive: the first copy could not be read back after the apply - " .. tostring(a.readError))
		return lines
	end

	local d = r.delete
	local deleted = type(d) == "table" and not d.skipped and d.ok and not d.existsAfter
	local after = r.after
	local first = deleted and type(after) == "table" and type(after.first) == "table" and after.first or nil
	local judged = first ~= nil and not first.readError

	-- A preset that changed nothing leaves no edit for the delete to keep, so
	-- an unchanged copy afterwards proves nothing.
	local contrastArrived = a.contrast == r.expectedContrast
	local applied = contrastArrived or a.maskFound
	local noEffect = string.format(
		"the preset had no visible effect on the first copy (Contrast2012 %s, subject mask missing)",
		tostring(a.contrast)
	)
	if not applied and not judged then
		add("E4f " .. noEffect .. ".")
	elseif applied and not contrastArrived then
		add(
			string.format(
				"E4f the preset's Contrast2012 did not arrive on the first copy (read back %s, the preset holds %s); the subject mask did, so the verdict rests on the mask alone.",
				tostring(a.contrast),
				tostring(r.expectedContrast)
			)
		)
	end

	if type(d) ~= "table" then
		return lines
	end
	if not deleted then
		add(
			"E4f inconclusive: the preset file "
				.. describeDelete(d)
				.. ". It stays in the list of preset files to delete by hand."
		)
	end

	if first ~= nil then
		if first.readError then
			add(
				"E4f inconclusive: the first copy could not be read back after the delete - "
					.. tostring(first.readError)
			)
		elseif not applied then
			add("E4f inconclusive: " .. noEffect .. ", so there was no edit for the delete to preserve.")
		else
			add(describeViability(r, a, first))
		end
	end

	if deleted and type(after) == "table" then
		local setting = after.setting or {}
		local settingText
		if setting.ok then
			settingText = "works (Contrast2012 " .. tostring(setting.contrast) .. ")"
		else
			settingText = "fails - " .. tostring(setting.error)
		end
		local secondText = "not tested (run stopped)"
		if type(r.second) == "table" then
			local second = {}
			for key, value in pairs(r.second) do
				second[key] = value
			end
			second.expectedContrast = r.expectedContrast
			secondText = describeSecondApply(second)
		end
		add(
			string.format(
				"E4f until Lightroom restarts, after the file was deleted: getDevelopPresetsForPlugin still lists it: %s (%s preset(s) of that name, same uuid listed: %s); lookup by uuid: %s; preset:getSetting(): %s; applying the deleted preset to a second copy: %s. What a restart changes is a manual check: the 'Plugin Develop Presets' folder, and E4 run again.",
				yesNo(after.listedByUuid or (tonumber(after.namedCount) or 0) > 0),
				tostring(after.namedCount),
				yesNo(after.listedByUuid),
				tostring(after.lookup),
				settingText,
				secondText
			)
		)
	end

	if type(r.readd) == "table" then
		add(describeReadd(r, r.readd))
	end
	return lines
end

--- Turns what E4f observed into verdict lines.
--
-- @param r table Built by the task step by step; a part is nil when the run
--   stopped (canceled or failed) before it:
--   `{ name, expectedContrast,
--      before = { { uuid, file, fileExists } },   -- presets of that name listed first
--      create = { ok, error },
--      apply = { ok, error, readError, contrast, maskFound, maskState,
--        maskPollState },                        -- maskPollState: the raw poll state
--      delete = { skipped, ok, error, existsAfter },
--      after = { namedCount, listedByUuid, lookup, setting = { ok, error,
--        contrast }, first = { readError, contrast, maskFound, maskState } },
--      second = { copyError, ok, error, readError, contrast, maskFound, fileBack },
--      readd = { ok, error, file, fileExists, samePath, sameUuid, contrast,
--        expectedContrast, fileContrast, fileHasNewContrast, namedCount,
--        cleanup = { skipped, ok, error, existsAfter } },
--      canceled }`
-- @return table Array of verdict strings.
function DevelopExperiments.describeE4f(r)
	if type(r) ~= "table" then
		return {}
	end
	local lines = describeE4fParts(r)
	if r.canceled then
		table.insert(
			lines,
			"E4f not finished: the run was canceled. A preset file it created and did not delete is in the list of files to delete by hand."
		)
	end
	return lines
end

local HEADLINE_WORDS = { "VIABLE", "NOT viable", "inconclusive", "not run", "not finished" }

--- Whether a verdict is an experiment's answer that the final dialog must
-- show even past its cap: today the E4f outcome, which E4 always produces
-- after a dozen other verdicts.
function DevelopExperiments.isHeadlineVerdict(verdict)
	if type(verdict) ~= "string" or verdict:sub(1, 4) ~= "E4f " then
		return false
	end
	for _, word in ipairs(HEADLINE_WORDS) do
		if verdict:find(word, 1, true) then
			return true
		end
	end
	return false
end

--- Picks the verdicts the final dialog shows: the first `cap`, plus every
-- headline verdict after them.
-- @return table, number The verdicts to show and how many are left out.
function DevelopExperiments.summaryVerdicts(verdicts, cap)
	local shown, hidden = {}, 0
	for index, verdict in ipairs(verdicts or {}) do
		if index <= cap or DevelopExperiments.isHeadlineVerdict(verdict) then
			table.insert(shown, verdict)
		else
			hidden = hidden + 1
		end
	end
	return shown, hidden
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
