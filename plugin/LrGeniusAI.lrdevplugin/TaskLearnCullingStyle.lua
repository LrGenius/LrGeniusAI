-- TaskLearnCullingStyle.lua
-- Learns how the photographer culls from flags and stars they set themselves
-- on the selected photos, so 'Cull Similar Photos' can rank bursts their way.
-- The backend learns and keeps the style (`/v1/cull/taste`); this task reads
-- the marks, tells the photographer's from the ones a cull wrote, and reports
-- how well the result agrees with them.

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskLearnCullingStyle", function(context)
		local title = "Learn My Culling Style"
		local current, statusErr = SearchIndexAPI.cullingStyleStatus()
		if statusErr then
			ErrorHandler.handleError(title, statusErr)
			return
		end

		local intro = "Select photos from shoots you culled yourself — flags, rejects and stars — and "
			.. "this learns which frame of a burst you prefer: sharper, the broader smile, the "
			.. "stronger moment. Marks a cull set and you left alone are not counted. Everything "
			.. "stays on this computer."
		if current then
			intro = intro .. "\n\nLearned so far: " .. Util.cullingStyleSummary(current)
		end
		local choice = LrDialogs.confirm(
			title,
			intro,
			"Learn from the selected photos",
			"Cancel",
			current and "Forget the learned style" or nil
		)
		if choice == "other" then
			local deleted, forgetErr = SearchIndexAPI.forgetCullingStyle()
			if forgetErr then
				ErrorHandler.handleError(title, forgetErr)
			else
				LrDialogs.message(
					title,
					deleted and "The learned culling style was forgotten." or "There was nothing to forget.",
					"info"
				)
			end
			return
		end
		if choice ~= "ok" then
			return
		end

		local photos = PhotoSelector.getPhotosInScope("selected")
		if not photos or #photos == 0 then
			LrDialogs.message(title, "Select the photos to learn from first.", "info")
			return
		end
		local photoById = {}
		local missingIds = 0
		for _, photo in ipairs(photos) do
			local photoId = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if photoId then
				photoById[photoId] = photo
			else
				missingIds = missingIds + 1
			end
		end
		local catalog = LrApplication.activeCatalog()
		local labels = Util.cullTasteLabels(Util.cullCurrentMarks(catalog, photoById))
		local own = 0
		for _, label in ipairs(labels) do
			if label.own then
				own = own + 1
			end
		end
		if own == 0 then
			LrDialogs.message(
				title,
				"None of the selected photos has a flag, a reject or stars of your own. Select photos "
					.. "from shoots you culled yourself.",
				"info"
			)
			return
		end

		local progress = LrProgressScope({ title = "Learning your culling style...", functionContext = context })
		progress:setPortionComplete(0, 1)
		local model, err = SearchIndexAPI.learnCullingStyle(labels, "auto")
		progress:done()
		if err then
			ErrorHandler.handleError(title, err)
			return
		end
		local message = Util.cullingStyleSummary(model)
		if missingIds > 0 then
			message = message .. string.format("\n\n%d photo(s) could not be identified and were left out.", missingIds)
		end
		LrDialogs.message(title, message, (model and model.used) and "info" or "warning")
	end)
end)
