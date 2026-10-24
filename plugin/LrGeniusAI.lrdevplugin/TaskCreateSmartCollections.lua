-- TaskCreateSmartCollections.lua
-- Library > Plug-in Extras > Create Smart Collection Templates...
--
-- Creates ready-made smart collections over LrGeniusAI's metadata fields
-- (best photos, quality flags, culling decisions, species, genres) in a
-- "LrGeniusAI" collection set. The templates themselves are in
-- SmartCollectionTemplates.lua; this file is the dialog and the catalog work.
--
-- Safe to run again: a template whose name is already taken in its set is left
-- as it is (the user may have edited it), and the run says so.

--- Reads the collections already in one child set: name -> "smart" | "regular".
local function existingCollections(collectionSet)
	local byName = {}
	if not collectionSet then
		return byName
	end
	for _, collection in ipairs(collectionSet:getChildCollections() or {}) do
		local okSmart, isSmart = LrTasks.pcall(function()
			return collection:isSmartCollection()
		end)
		byName[collection:getName()] = (okSmart and isSmart) and "smart" or "regular"
	end
	return byName
end

--- The selection dialog. Returns the set of ticked template ids, or nil when
--- the user cancelled.
local function showDialog(context, templates, genresNote)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local props = LrBinding.makePropertyTable(context)

	-- One checkbox per template, keyed by its position: template ids contain
	-- dots, which a property key should not.
	local keyOf = {}
	for index, template in ipairs(templates) do
		local key = "template" .. tostring(index)
		keyOf[template.id] = key
		props[key] = true
	end

	local function setAll(value)
		for _, key in pairs(keyOf) do
			props[key] = value
		end
	end

	local function groupBox(group)
		local rows = {}
		for _, template in ipairs(templates) do
			if template.group == group.id then
				rows[#rows + 1] = f:checkbox({
					title = template.name,
					value = bind(keyOf[template.id]),
					tooltip = "Filled by " .. template.requires .. ".",
				})
			end
		end
		if #rows == 0 then
			return nil
		end
		rows.title = group.title
		rows.fill_horizontal = 1
		return f:group_box(rows)
	end

	local left = { spacing = f:control_spacing(), fill_vertical = 1 }
	local right = { spacing = f:control_spacing(), fill_vertical = 1 }
	for _, group in ipairs(SmartCollectionTemplates.GROUPS) do
		local box = groupBox(group)
		if box then
			if group.id == "genre" then
				right[#right + 1] = box
			else
				left[#left + 1] = box
			end
		end
	end

	local contents = {
		bind_to_object = props,
		spacing = f:control_spacing(),
		f:static_text({
			title = 'Smart collections are created in a "'
				.. SmartCollectionTemplates.ROOT_SET
				.. '" collection set, one set per group. They update by themselves as LrGeniusAI fills in its metadata fields.',
			width_in_chars = 70,
			height_in_lines = 2,
		}),
		f:row({ spacing = f:control_spacing(), f:column(left), f:column(right) }),
		f:row({
			f:push_button({
				title = "Select all",
				action = function()
					setAll(true)
				end,
			}),
			f:push_button({
				title = "Select none",
				action = function()
					setAll(false)
				end,
			}),
		}),
	}
	if genresNote then
		contents[#contents + 1] = f:static_text({
			title = genresNote,
			width_in_chars = 70,
			height_in_lines = 2,
		})
	end

	local result = LrDialogs.presentModalDialog({
		title = "Create Smart Collection Templates",
		contents = f:column(contents),
		actionVerb = "Create",
	})
	if result ~= "ok" then
		return nil
	end

	local selected = {}
	for id, key in pairs(keyOf) do
		if props[key] then
			selected[id] = true
		end
	end
	return selected
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskCreateSmartCollections", function(context)
		LrDialogs.attachErrorDialogToFunctionContext(context)

		-- The genre labels are the backend's. Everything else works without
		-- it, so an unreachable backend costs the genre group only — and the
		-- dialog says so instead of leaving the group silently missing.
		local genres, genresNote = nil, nil
		if SearchIndexAPI.pingServer() then
			genres = SearchIndexAPI.getSearchGenres()
		end
		if type(genres) ~= "table" or #genres == 0 then
			genres = nil
			genresNote = "The genre collections are missing: the LrGeniusAI backend did not answer, and the genre "
				.. "names come from it. Start it (Plug-in Manager > LrGeniusAI) and run this again to add them."
		end

		local templates = SmartCollectionTemplates.build(_PLUGIN.id, genres)
		local selected = showDialog(context, templates, genresNote)
		if not selected then
			return
		end
		if next(selected) == nil then
			LrDialogs.message("Nothing selected", "Tick at least one smart collection to create.", "info")
			return
		end

		local catalog = LrApplication.activeCatalog()
		local wantedGroups = {}
		for _, template in ipairs(templates) do
			if selected[template.id] then
				wantedGroups[template.group] = true
			end
		end

		-- The sets first, in a transaction of their own: a set created inside
		-- a write-access block cannot be asked for its children in that same
		-- block, and the children are what tells a re-run what already exists.
		local setsByGroup = {}
		local setsWrite = catalog:withWriteAccessDo("Create LrGeniusAI collection sets", function()
			local root = catalog:createCollectionSet(SmartCollectionTemplates.ROOT_SET, nil, true)
			if not root then
				return
			end
			for _, group in ipairs(SmartCollectionTemplates.GROUPS) do
				if wantedGroups[group.id] then
					setsByGroup[group.id] = catalog:createCollectionSet(group.title, root, true)
				end
			end
		end, Defaults.catalogWriteAccessOptions)
		if setsWrite == "aborted" then
			ErrorHandler.handleError(
				"Smart collections not created",
				"The catalog was busy and the write timed out. Nothing was changed; run this again."
			)
			return
		end
		local missingSets = {}
		for _, group in ipairs(SmartCollectionTemplates.GROUPS) do
			if wantedGroups[group.id] and not setsByGroup[group.id] then
				missingSets[#missingSets + 1] = group.title
			end
		end
		if #missingSets > 0 then
			ErrorHandler.handleError(
				"Smart collections not created",
				'Lightroom did not create the collection set(s) "'
					.. SmartCollectionTemplates.ROOT_SET
					.. '" / '
					.. table.concat(missingSets, ", ")
					.. ". If a collection (not a set) of that name exists at the top level of the Collections "
					.. "panel, rename it and run this again."
			)
			return
		end

		local existing = {}
		for groupId, collectionSet in pairs(setsByGroup) do
			existing[groupId] = existingCollections(collectionSet)
		end
		local plan = SmartCollectionTemplates.plan(templates, selected, existing)

		local created, failed = 0, {}
		if #plan.create > 0 then
			local write = catalog:withWriteAccessDo("Create LrGeniusAI smart collections", function()
				for _, template in ipairs(plan.create) do
					local ok, collection = LrTasks.pcall(function()
						return catalog:createSmartCollection(
							template.name,
							template.searchDesc,
							setsByGroup[template.group],
							true
						)
					end)
					if ok and collection then
						created = created + 1
					else
						log:error(
							"createSmartCollection failed for "
								.. template.id
								.. ": "
								.. tostring(collection)
								.. " searchDesc="
								.. Util.dumpTable(template.searchDesc)
						)
						failed[#failed + 1] = template.name
					end
				end
			end, Defaults.catalogWriteAccessOptions)
			if write == "aborted" then
				ErrorHandler.handleError(
					"Smart collections not created",
					"The catalog was busy and the write timed out. The collection sets exist; run this again to fill them."
				)
				return
			end
		end

		local conflictNames = {}
		for _, template in ipairs(plan.conflicts) do
			conflictNames[#conflictNames + 1] = template.name
		end
		local message = SmartCollectionTemplates.summary({
			created = created,
			kept = #plan.kept,
			conflicts = conflictNames,
			failed = failed,
		})
		if genresNote then
			message = message .. "\n\n" .. genresNote
		end
		local degraded = #failed > 0 or #conflictNames > 0 or genresNote ~= nil
		LrDialogs.message("Smart Collection Templates", message, degraded and "warning" or "info")
	end)
end)
