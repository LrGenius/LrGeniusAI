-- TaskTrainFromEdits.lua
-- Allows the user to save their current Lightroom develop settings for selected
-- photos as AI style training examples.  These are stored on the backend and
-- injected as few-shot context the next time AI Edit Photos runs.

require("DevelopEditManager")

local function showTrainDialog(ctx)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local props = LrBinding.makePropertyTable(ctx)

	props.label = prefs.trainingLabel or ""
	props.summary = prefs.trainingSummary or ""
	props.scope = prefs.trainingScope or "selected"

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/AnalyzeAndIndex/Scope=Scope"),
					width = 150,
				}),
				f:popup_menu({
					value = bind("scope"),
					width = 300,
					items = {
						{ title = LOC("$$$/LrGeniusAI/common/ScopeSelected=Selected photos only"), value = "selected" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeView=Current view"), value = "view" },
						{ title = LOC("$$$/LrGeniusAI/common/ScopeAll=Entire Catalog"), value = "all" },
					},
				}),
			}),
		}),
		f:group_box({
			title = LOC("$$$/LrGeniusAI/Training/StyleGroup=Edit Style"),
			fill_horizontal = 1,
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/Training/LabelLabel=Style label (optional):"),
					width = 180,
				}),
				f:edit_field({
					value = bind("label"),
					width_in_chars = 30,
					placeholder_string = LOC("$$$/LrGeniusAI/Training/LabelPlaceholder=e.g. Wedding, Portrait, Street"),
				}),
			}),
			f:row({
				f:static_text({
					title = LOC("$$$/LrGeniusAI/Training/SummaryLabel=Description (optional):"),
					width = 180,
				}),
				f:edit_field({
					value = bind("summary"),
					width_in_chars = 30,
					height_in_lines = 2,
				}),
			}),
		}),
		f:row({
			f:static_text({
				title = LOC(
					"$$$/LrGeniusAI/Training/DialogHint=Hint: Only select photos that you have manually edited. The AI will learn your style from these examples."
				),
				font = "italic",
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/Training/DialogTitle=Save Edits as AI Training Examples"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/Training/SaveButton=Save Examples"),
	})

	if result ~= "ok" then
		return nil
	end

	prefs.trainingLabel = props.label
	prefs.trainingSummary = props.summary
	prefs.trainingScope = props.scope

	return {
		label = props.label,
		summary = props.summary,
		scope = props.scope,
	}
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TrainFromEditsTask", function(ctx)
		LrDialogs.attachErrorDialogToFunctionContext(ctx)
		log:info("Save Training Examples task started")

		if not Util.waitForServerDialog() then
			log:warn("Train task aborted: backend server unavailable")
			return
		end

		local options = showTrainDialog(ctx)
		if not options then
			log:info("Train task cancelled by user")
			return
		end

		local photosToProcess = PhotoSelector.getPhotosInScope(options.scope)
		if not photosToProcess or #photosToProcess == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/Training/NoPhotosTitle=No Photos"),
				LOC("$$$/LrGeniusAI/Training/NoPhotosMsg=No photos found in the selected scope."),
				"info"
			)
			return
		end

		-- Filter photos: only RAW or DNG formats.
		local photos = {}
		for _, photo in ipairs(photosToProcess) do
			local fmt = photo:getRawMetadata("fileFormat")

			-- Only include RAW or DNG.
			if fmt == "RAW" or fmt == "DNG" then
				table.insert(photos, photo)
			end
		end

		if #photos == 0 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/Training/NoValidPhotosTitle=No Valid Training Photos"),
				LOC(
					"$$$/LrGeniusAI/Training/NoValidPhotosMsg=None of the photos in the selected scope match the training criteria (must be RAW or DNG format). JPEGs, TIFFs, and other formats are excluded."
				),
				"info"
			)
			return
		end

		local progressScope = LrProgressScope({
			title = LOC("$$$/LrGeniusAI/Training/Progress=Saving training examples..."),
			functionContext = ctx,
		})
		progressScope:setPortionComplete(0, #photos)

		local successCount = 0
		local errorCount = 0
		local errorMessages = {}
		local backendWarnings = {}

		for index, photo in ipairs(photos) do
			if progressScope:isCanceled() then
				break
			end

			local fileName = photo:getFormattedMetadata("fileName") or "Photo"
			progressScope:setCaption(
				string.format(
					LOC("$$$/LrGeniusAI/Training/ProgressCaption=Processing %s (%d of %d)"),
					fileName,
					index,
					#photos
				)
			)
			progressScope:setPortionComplete(index - 1, #photos)

			-- Read current develop settings.
			local developSettings
			local okGet, devOrErr = LrTasks.pcall(function()
				return photo:getDevelopSettings()
			end)
			if okGet and type(devOrErr) == "table" then
				developSettings = devOrErr
			else
				log:warn("Could not read develop settings for " .. fileName .. ": " .. tostring(devOrErr))
				developSettings = {}
			end

			-- Get a stable photo ID.
			local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
			if not photoId then
				log:error("Failed to resolve photo ID for " .. fileName .. ": " .. tostring(photoIdErr))
				table.insert(errorMessages, fileName .. ": " .. tostring(photoIdErr))
				errorCount = errorCount + 1
			else
				-- Collect EXIF metadata for richer style matching using standardized utility.
				local exifOptions = Util.getPhotoExif(photo)
				exifOptions.label = options.label
				exifOptions.summary = options.summary

				-- Export a JPEG thumbnail for CLIP embedding + exposure analysis.
				local exportedPath = SearchIndexAPI.exportPhotoForIndexing(photo)

				local ok, resp = SearchIndexAPI.addTrainingExample(
					photoId,
					exportedPath, -- may be nil; server will still store settings
					developSettings,
					exifOptions
				)

				-- Clean up temp file.
				if exportedPath then
					LrTasks.pcall(function()
						if LrFileUtils.exists(exportedPath) then
							LrFileUtils.delete(exportedPath)
						end
					end)
				end

				if ok then
					successCount = successCount + 1
					log:info("Saved training example for " .. fileName)
					if resp and resp.warning then
						table.insert(backendWarnings, fileName .. ": " .. tostring(resp.warning))
					end
				else
					errorCount = errorCount + 1
					table.insert(errorMessages, fileName .. ": " .. tostring(resp))
					log:error("Failed to save training example for " .. fileName .. ": " .. tostring(resp))
				end
			end

			progressScope:setPortionComplete(index, #photos)
		end

		progressScope:done()

		-- Summary dialog.
		if errorCount > 0 or #backendWarnings > 0 then
			local uniqueErrors = {}
			local errorList = {}
			for _, msg in ipairs(errorMessages) do
				if not uniqueErrors[msg] then
					uniqueErrors[msg] = true
					table.insert(errorList, "- " .. msg)
					if #errorList >= 5 then
						break
					end
				end
			end

			local combinedReport =
				LOC("$$$/LrGeniusAI/Training/Summary=Saved ^1 training example(s).", tostring(successCount))
			if errorCount > 0 then
				combinedReport = combinedReport
					.. "\n"
					.. LOC("$$$/LrGeniusAI/common/Errors=Errors: ^1", tostring(errorCount))
			end

			if #errorList > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. LOC("$$$/LrGeniusAI/common/ErrorDetails=Error details:")
					.. "\n"
					.. table.concat(errorList, "\n")
				if #errorMessages > 5 then
					combinedReport = combinedReport
						.. "\n"
						.. LOC("$$$/LrGeniusAI/common/MoreErrors=... and ^1 more errors", tostring(#errorMessages - 5))
				end
			end

			if #backendWarnings > 0 then
				combinedReport = combinedReport
					.. "\n\n"
					.. LOC("$$$/LrGeniusAI/common/BackendWarnings=Backend Warnings:")
					.. "\n"
				for i = 1, math.min(5, #backendWarnings) do
					combinedReport = combinedReport .. "- " .. backendWarnings[i] .. "\n"
				end
				if #backendWarnings > 5 then
					combinedReport = combinedReport
						.. LOC(
							"$$$/LrGeniusAI/common/MoreWarnings=... and ^1 more warnings",
							tostring(#backendWarnings - 5)
						)
				end
			end

			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/Training/CompletionTitle=Training Examples Saved"),
				combinedReport
			)
		else
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/Training/SuccessTitle=Training Examples Saved"),
				LOC(
					"$$$/LrGeniusAI/Training/SuccessSummary=Successfully saved ^1 training example(s).\nAI Edit Photos will use your style when editing visually similar photos.",
					tostring(successCount)
				),
				"info"
			)
		end
	end)
end)
