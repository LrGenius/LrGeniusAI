--[[
    LightroomPreviews.lua

    Rendered JPEGs of photos without an export: Lightroom's own previews,
    through `photo:requestJpegThumbnail`.

    An export renders the photo from scratch, which is what makes indexing a
    large selection slow and what lets Lightroom's memory climb over a long
    run. A preview is already rendered — with the photo's current develop
    settings — whenever Lightroom has one cached, so handing it over costs
    almost nothing. Two things make it unreliable, and both are handled here
    rather than by every caller:

    * Lightroom answers with whatever it has cached, which can be far smaller
      than what was asked for (a 160 px grid thumbnail when only that was ever
      built). The size is read from the JPEG itself (`Util.jpegDimensions`)
      and the caller decides what is big enough; anything smaller is the
      caller's cue to export instead.
    * The callback may never come (a photo whose preview cannot be built), or
      come more than once (a small preview first, a better one after). The
      wait has a deadline, and a better answer arriving shortly after a first
      one is still taken.

    Previews can lag behind the latest edit until Lightroom re-renders them;
    for a search preview that is fine, for an aesthetic score it is the
    trade-off the "Use Lightroom previews" setting names.
]]

LightroomPreviews = {}

--- How long to keep waiting for a better answer once a first one arrived.
local SETTLE_SECONDS = 0.4

---
-- Asks Lightroom for a rendered JPEG of `photo`, at most `longEdge` px on its
-- long edge.
-- @param photo LrPhoto
-- @param longEdge number The size wanted.
-- @param timeoutSeconds number|nil How long to wait for Lightroom (default 10).
-- @return string|nil jpegData, number longEdgeReceived (0 when none),
--   string|nil reason Why there is no data, for the log.
--
function LightroomPreviews.requestJpeg(photo, longEdge, timeoutSeconds)
	local deadline = LrDate.currentTime() + (timeoutSeconds or 10)
	local best, bestEdge, failure = nil, 0, nil
	local firstAt = nil

	local ok, request = LrTasks.pcall(function()
		return photo:requestJpegThumbnail(longEdge, longEdge, function(data, err)
			if type(data) == "string" and #data > 0 then
				local w, h = Util.jpegDimensions(data)
				local edge = math.max(w or 0, h or 0)
				if edge > bestEdge then
					best, bestEdge = data, edge
				end
				firstAt = firstAt or LrDate.currentTime()
			else
				failure = err or "Lightroom returned no preview"
			end
		end)
	end)
	if not ok then
		return nil, 0, "requestJpegThumbnail failed: " .. tostring(request)
	end

	-- `request` must stay referenced until the callback has run: Lightroom
	-- cancels a request whose handle is garbage-collected.
	while LrDate.currentTime() < deadline do
		if bestEdge >= longEdge then
			break
		end
		if firstAt and LrDate.currentTime() - firstAt >= SETTLE_SECONDS then
			break
		end
		if failure and not best then
			break
		end
		LrTasks.sleep(0.05)
	end
	request = nil -- luacheck: ignore 311

	if best == nil then
		return nil, 0, failure or "Lightroom did not answer in time"
	end
	return best, bestEdge, nil
end

---
-- Writes JPEG bytes to a new file in the temp folder.
-- @return string|nil path, string|nil error
--
function LightroomPreviews.writeTemp(data, nameHint)
	local name = "lrgenius-preview-"
		.. tostring(nameHint or "photo"):gsub("[^%w%-_]", "_")
		.. "-"
		.. tostring(math.floor(LrDate.currentTime() * 1000))
		.. ".jpg"
	local path = LrPathUtils.child(LrPathUtils.getStandardFilePath("temp"), name)
	local fh, err = io.open(path, "wb")
	if not fh then
		return nil, "could not write " .. path .. ": " .. tostring(err)
	end
	fh:write(data)
	fh:close()
	return path, nil
end

---
-- A rendered JPEG of `photo` on disk, from its Lightroom preview when that is
-- at least `minLongEdge` px, else nil (the caller exports instead).
-- @return string|nil path, string|nil reason Why the preview was not used.
--
function LightroomPreviews.previewFile(photo, wantedLongEdge, minLongEdge, timeoutSeconds)
	local data, edge, reason = LightroomPreviews.requestJpeg(photo, wantedLongEdge, timeoutSeconds)
	if data == nil then
		return nil, reason
	end
	if edge < minLongEdge then
		return nil, "the cached preview is " .. tostring(edge) .. " px, " .. tostring(minLongEdge) .. " px needed"
	end
	return LightroomPreviews.writeTemp(data, photo.localIdentifier)
end

return LightroomPreviews
