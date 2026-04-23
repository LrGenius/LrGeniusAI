--[[
    People: list face clusters (persons), assign names, and show photos in Library.
]]

--- Decodiert Base64-JPEG in eine Temp-Datei (für Lazy-Load). Gibt Pfad oder nil zurück.
local function writePersonThumbnailFile(base64Thumb, personId, index)
	if not base64Thumb or base64Thumb == "" then
		return nil
	end
	local tempDir = LrPathUtils.getStandardFilePath("temp")
	local safeId = (personId and personId ~= "") and personId or ("person_" .. tostring(index))
	local safeIdClean = safeId:gsub("[^%w_-]", "_")
	local tempFile = LrPathUtils.child(tempDir, "lrgenius_person_" .. safeIdClean .. ".jpg")
	local fh = io.open(tempFile, "wb")
	if fh then
		fh:write(LrStringUtils.decodeBase64(base64Thumb))
		fh:close()
		return tempFile
	end
	return nil
end

--- Minimal 1×1-JPEG als Platzhalter, bis echte Thumbnails geladen sind (gebunden an f:picture).
local _thumbPlaceholderPath
local function ensureThumbPlaceholderPath()
	if _thumbPlaceholderPath then
		return _thumbPlaceholderPath
	end
	local tempDir = LrPathUtils.getStandardFilePath("temp")
	local path = LrPathUtils.child(tempDir, "lrgenius_person_thumb_placeholder.jpg")
	local fh = io.open(path, "wb")
	if fh then
		local tiny =
			"/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAv/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFQEBAQAAAAAAAAAAAAAAAAAAAAX/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIRAxEAPwCwAA8A/9k="
		fh:write(LrStringUtils.decodeBase64(tiny))
		fh:close()
		_thumbPlaceholderPath = path
	end
	return _thumbPlaceholderPath or ""
end

--- Namen zuerst (nach photo_count absteigend), dann Unbenannte (nach photo_count absteigend).
local function sortPersonsForDisplay(persons)
	if not persons or #persons < 2 then
		return
	end
	local function hasName(p)
		return p and type(p.name) == "string" and p.name ~= ""
	end
	local function photoCount(p)
		return tonumber(p and p.photo_count) or 0
	end
	table.sort(persons, function(a, b)
		local aNamed, bNamed = hasName(a), hasName(b)
		if aNamed ~= bNamed then
			return aNamed
		end
		return photoCount(a) > photoCount(b)
	end)
end

--- Lädt Personenliste vom Server (ohne Thumbnails; die werden im Dialog per Lazy-Load geholt).
local function loadPersonsFromServer()
	local resp, err = SearchIndexAPI.getPersons()
	if err then
		return {},
			(LOC(
				"$$$/LrGeniusAI/People/LoadError=Could not load persons. Check server connection. Try 'Cluster faces' or close and reopen."
			))
	end

	if resp and resp.warning then
		LrDialogs.message(LOC("$$$/LrGeniusAI/common/BackendWarning=Backend Warning"), resp.warning, "warning")
	end

	local persons = (resp and resp.persons) and resp.persons or {}
	sortPersonsForDisplay(persons)
	return persons, nil
end

--- Zeigt den Personen-Dialog. persons ohne Thumbnails; Thumbnails per GET /faces/persons/<id>/thumbnail (Lazy-Load im Hintergrund).
-- Footer: actionVerb=Save, cancelVerb=Cancel, otherVerb=Reset (Felder zurück auf Snapshot). Namen speichern bei "ok".
local function showPeopleDialog(ctx, persons, loadError)
	local f = LrView.osFactory()
	local bind = LrView.bind
	local share = LrView.share

	persons = persons or {}

	local props = LrBinding.makePropertyTable(ctx)
	props.persons = persons
	props.libraryMatchMode = "intersection"

	local nameSnapshot = {}
	if #persons > 0 then
		local ph = ensureThumbPlaceholderPath()
		for idx = 1, #persons do
			props["personThumb_" .. idx] = ph
			local p = persons[idx]
			local nm = (p and type(p.name) == "string") and p.name or ""
			nameSnapshot[idx] = nm
			props["personName_" .. idx] = nm
			if p and p.person_id and p.person_id ~= "" then
				props["librarySel_" .. idx] = false
			end
		end
	end

	local pendingShowInLibrary = nil

	local function buildLibrarySelection()
		local list = {}
		for idx = 1, #persons do
			local p = persons[idx]
			if p and p.person_id and p.person_id ~= "" and props["librarySel_" .. idx] then
				local nm = props["personName_" .. idx]
				local personName = (type(nm) == "string" and nm ~= "") and nm or nil
				list[#list + 1] = { person_id = p.person_id, person_name = personName }
			end
		end
		return list
	end

	local GRID_COLS = 4
	local THUMB_SIZE = 96

	local function photoCountLabel(pc)
		pc = tonumber(pc) or 0
		local unit = (pc == 1) and (LOC("$$$/LrGeniusAI/People/Photo=photo"))
			or (LOC("$$$/LrGeniusAI/People/Photos=photos"))
		return string.format("%d %s", pc, unit)
	end

	local listScroller
	local peopleListBlock
	if #persons == 0 then
		peopleListBlock = f:static_text({
			title = loadError
				or LOC(
					"$$$/LrGeniusAI/People/NoPersons=No persons yet. Run 'Cluster faces' after indexing photos with face embeddings."
				),
		})
		listScroller = peopleListBlock
	else
		local gridRows = {}
		for startIdx = 1, #persons, GRID_COLS do
			local rowCells = {}
			for c = 0, GRID_COLS - 1 do
				local idx = startIdx + c
				if idx <= #persons then
					local p = persons[idx]
					local thumbKey = "personThumb_" .. idx
					local nameKey = "personName_" .. idx
					local thumbView = f:picture({
						alignment = "center",
						value = bind(thumbKey),
						width = THUMB_SIZE,
						height = THUMB_SIZE,
					})
					local nameRow
					if p and p.person_id and p.person_id ~= "" then
						nameRow = f:edit_field({
							value = bind(nameKey),
							width_in_chars = 14,
							immediate = true,
						})
					else
						nameRow = f:static_text({
							title = LOC("$$$/LrGeniusAI/People/Unnamed=Unnamed"),
							alignment = "center",
						})
					end
					local libRow
					if p and p.person_id and p.person_id ~= "" then
						libRow = f:checkbox({
							value = bind("librarySel_" .. idx),
							title = LOC("$$$/LrGeniusAI/People/SelectForLibrary=Library"),
						})
					else
						libRow = f:spacer({ height = 1 })
					end
					rowCells[#rowCells + 1] = f:column({
						spacing = 6,
						width = share("personCell"),
						alignment = "center",
						thumbView,
						nameRow,
						f:static_text({
							title = photoCountLabel(p.photo_count),
							size = "small",
							alignment = "center",
						}),
						libRow,
					})
				else
					rowCells[#rowCells + 1] = f:spacer({ width = share("personCell") })
				end
			end
			gridRows[#gridRows + 1] = f:row({
				spacing = 14,
				alignment = "center",
				unpack(rowCells),
			})
		end

		listScroller = f:scrolled_view({
			horizontal_scroller = false,
			vertical_scroller = true,
			width = 740,
			height = 320,
			alignment = "center",
			f:column({
				spacing = 12,
				unpack(gridRows),
			}),
		})

		peopleListBlock = f:group_box({
			title = LOC("$$$/LrGeniusAI/People/TableGroupTitle=People"),
			fill_horizontal = 1,
			listScroller,
		})
	end

	local contents = f:column({
		bind_to_object = props,
		spacing = f:control_spacing(),
		fill_horizontal = 1,

		f:row({
			spacing = f:control_spacing(),
			f:push_button({
				title = LOC("$$$/LrGeniusAI/People/ClusterFaces=Cluster faces"),
				action = function()
					local clusterResp, err = SearchIndexAPI.clusterFaces()
					if err then
						ErrorHandler.handleError(LOC("$$$/LrGeniusAI/People/ClusterError=Face clustering failed"), err)
						return
					end

					if clusterResp and clusterResp.warning then
						LrDialogs.message(
							LOC("$$$/LrGeniusAI/common/BackendWarning=Backend Warning"),
							clusterResp.warning,
							"warning"
						)
					end

					LrDialogs.message(
						LOC("$$$/LrGeniusAI/People/ClusterDone=Clustering done"),
						LOC(
							"$$$/LrGeniusAI/People/ClusterSummaryAndReopen=^1 persons, ^2 faces. Close this dialog and open 'People...' again to see the updated list.",
							tostring(clusterResp and clusterResp.person_count or 0),
							tostring(clusterResp and clusterResp.face_count or 0)
						)
					)
				end,
			}),
			f:push_button({
				title = LOC("$$$/LrGeniusAI/People/ShowInLibrary=Show in Library"),
				action = function()
					local sel = buildLibrarySelection()
					if #sel == 0 then
						LrDialogs.message(
							LOC("$$$/LrGeniusAI/People/NoLibrarySelectionTitle=No people selected"),
							LOC(
								"$$$/LrGeniusAI/People/NoLibrarySelectionMessage=Check Library on one or more people, then try again."
							)
						)
						return
					end
					pendingShowInLibrary = {
						entries = sel,
						matchMode = props.libraryMatchMode or "intersection",
					}
					LrDialogs.stopModalWithResult(listScroller, "show_library")
				end,
			}),
		}),

		f:row({
			spacing = f:control_spacing(),
			f:static_text({
				title = LOC("$$$/LrGeniusAI/People/LibraryMatchLabel=When several people are selected:"),
				width_in_chars = 34,
				alignment = "right",
			}),
			f:popup_menu({
				value = bind("libraryMatchMode"),
				items = {
					{
						title = LOC("$$$/LrGeniusAI/People/LibraryMatchOneOf=Photos with any selected person"),
						value = "union",
					},
					{
						title = LOC("$$$/LrGeniusAI/People/LibraryMatchAll=Photos with all selected people"),
						value = "intersection",
					},
				},
			}),
		}),

		f:static_text({
			title = LOC(
				"$$$/LrGeniusAI/People/ListTitle=Check Library for people to include, then Show in Library. Edit names; Save (OK) writes to the server, Reset reverts edits, Cancel closes without saving."
			),
			font = "<system/bold>",
		}),

		peopleListBlock,
	})

	local thumbLoaderDone = false
	if #persons > 0 then
		LrTasks.startAsyncTask(function()
			for idx = 1, #persons do
				if thumbLoaderDone then
					return
				end
				local p = persons[idx]
				if p and p.person_id and p.person_id ~= "" then
					local resp = SearchIndexAPI.getPersonThumbnail(p.person_id)
					if not thumbLoaderDone and resp and type(resp.thumbnail) == "string" and resp.thumbnail ~= "" then
						local path = writePersonThumbnailFile(resp.thumbnail, p.person_id, idx)
						if path and not thumbLoaderDone then
							props["personThumb_" .. idx] = path
						end
					end
				end
				LrTasks.yield()
			end
		end)
	end

	-- Lightroom SDK: actionVerb = primary OK (Save); cancelVerb; otherVerb (Reset).
	local dialogResult = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/People/WindowTitle=People"),
		contents = contents,
		actionVerb = LOC("$$$/LrGeniusAI/common/Save=Save"),
		cancelVerb = LOC("$$$/LrGeniusAI/common/Cancel=Cancel"),
		otherVerb = LOC("$$$/LrGeniusAI/People/Reset=Reset"),
	})
	thumbLoaderDone = true

	if
		dialogResult == "show_library"
		and type(pendingShowInLibrary) == "table"
		and type(pendingShowInLibrary.entries) == "table"
		and #pendingShowInLibrary.entries > 0
	then
		return "show_library", pendingShowInLibrary
	end

	if dialogResult == "ok" then
		for i = 1, #persons do
			local per = persons[i]
			if per and per.person_id and per.person_id ~= "" then
				local newName = props["personName_" .. i] or ""
				local oldName = nameSnapshot[i] or ""
				if newName ~= oldName then
					local nameOk, nameErr = SearchIndexAPI.setPersonName(per.person_id, newName)
					if not nameOk then
						ErrorHandler.handleError(LOC("$$$/LrGeniusAI/People/SetNameError=Could not set name"), nameErr)
					else
						per.name = newName
						nameSnapshot[i] = newName
					end
				end
			end
		end
		return "ok"
	end

	if dialogResult == "other" then
		return "reset"
	end

	return "cancel"
end

--- Baut sortierte Foto-ID-Liste aus API-Antwort.
local function photoIdsFromPersonResponse(resp)
	if type(resp) ~= "table" then
		return {}
	end
	return type(resp.photo_ids) == "table" and resp.photo_ids
		or (type(resp.photo_uuids) == "table" and resp.photo_uuids or {})
end

--- Union: jedes Foto, das mindestens eine der Personen enthält (Reihenfolge: erstes Auftreten).
local function unionPhotoIdsForEntries(entries)
	local seen = {}
	local photoIdsOrdered = {}
	for _, ent in ipairs(entries) do
		local pid = ent and ent.person_id
		if pid and pid ~= "" then
			local resp, err = SearchIndexAPI.getPhotosForPerson(pid)
			if err or type(resp) ~= "table" then
				ErrorHandler.handleError(
					LOC("$$$/LrGeniusAI/People/GetPhotosError=Could not get photos for person"),
					err or "No data"
				)
				return nil
			end

			if resp and resp.warning then
				log:warn("GetPhotosForPerson warning: " .. tostring(resp.warning))
			end

			local ids = photoIdsFromPersonResponse(resp)
			for _, photoId in ipairs(ids) do
				if not seen[photoId] then
					seen[photoId] = true
					table.insert(photoIdsOrdered, photoId)
				end
			end
		end
	end
	return photoIdsOrdered
end

--- Schnittmenge: Fotos, in denen alle Personen gemeinsam vorkommen (Reihenfolge wie erste Person).
local function intersectPhotoIdsForEntries(entries)
	if #entries == 0 then
		return {}
	end
	local sets = {}
	local firstIds = nil
	for _, ent in ipairs(entries) do
		local pid = ent and ent.person_id
		if not pid or pid == "" then
			return {}
		end
		local resp, err = SearchIndexAPI.getPhotosForPerson(pid)
		if err or type(resp) ~= "table" then
			ErrorHandler.handleError(
				LOC("$$$/LrGeniusAI/People/GetPhotosError=Could not get photos for person"),
				err or "No data"
			)
			return nil
		end
		local ids = photoIdsFromPersonResponse(resp)
		if not firstIds then
			firstIds = ids
		end
		local t = {}
		for _, id in ipairs(ids) do
			t[id] = true
		end
		sets[#sets + 1] = t
	end
	if #sets == 0 then
		return {}
	end
	local acc = sets[1]
	for i = 2, #sets do
		local nxt = sets[i]
		local newAcc = {}
		for id in pairs(acc) do
			if nxt[id] then
				newAcc[id] = true
			end
		end
		acc = newAcc
	end
	local ordered = {}
	for _, id in ipairs(firstIds) do
		if acc[id] then
			table.insert(ordered, id)
		end
	end
	return ordered
end

--- Führt "Show in Library" aus. entries: { person_id, person_name? }[]; matchMode: "union" | "intersection".
local function doShowInLibrary(entries, matchMode)
	if not entries or #entries == 0 then
		LrDialogs.message(
			LOC("$$$/LrGeniusAI/People/NoLibrarySelectionTitle=No people selected"),
			LOC("$$$/LrGeniusAI/People/NoLibrarySelectionMessage=Check Library on one or more people, then try again.")
		)
		return
	end

	local mode = (matchMode == "intersection") and "intersection" or "union"
	local photoIdsOrdered
	if mode == "intersection" and #entries >= 2 then
		photoIdsOrdered = intersectPhotoIdsForEntries(entries)
	else
		photoIdsOrdered = unionPhotoIdsForEntries(entries)
	end

	if photoIdsOrdered == nil then
		return
	end

	if #photoIdsOrdered == 0 then
		if mode == "intersection" and #entries >= 2 then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/People/NoPhotos=No photos"),
				LOC("$$$/LrGeniusAI/People/NoPhotosIntersection=No photos contain all selected people together.")
			)
		else
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/People/NoPhotos=No photos"),
				LOC("$$$/LrGeniusAI/People/NoPhotosForPerson=No photos found for this person.")
			)
		end
		return
	end

	local catalog = LrApplication.activeCatalog()
	local photos = SearchIndexAPI.findPhotosByPhotoIds(photoIdsOrdered)
	if #photos == 0 then
		LrDialogs.message(
			LOC("$$$/LrGeniusAI/People/NoPhotosInCatalog=Not in catalog"),
			LOC("$$$/LrGeniusAI/People/PersonPhotosNotInCatalog=Photos for this person are not in the current catalog.")
		)
		return
	end

	local nameParts = {}
	for _, ent in ipairs(entries) do
		local n = ent and ent.person_name
		if type(n) == "string" and n ~= "" then
			nameParts[#nameParts + 1] = n
		elseif ent and ent.person_id and ent.person_id ~= "" then
			nameParts[#nameParts + 1] = ent.person_id
		end
	end
	local label = table.concat(nameParts, ", ")
	if #label > 100 then
		label = string.format("%s (%d)", LOC("$$$/LrGeniusAI/People/MultiPeopleLabel=People"), #entries)
	end
	local collectionName = string.format("%s @ %s", label, LrDate.timeToW3CDate(LrDate.currentTime()))

	local collectionSet, collection
	catalog:withWriteAccessDo("Create Collection Set", function()
		collectionSet = catalog:createCollectionSet(LOC("$$$/LrGeniusAI/People/CollectionSetName=People"), nil, true)
	end, Defaults.catalogWriteAccessOptions)
	if not collectionSet then
		ErrorHandler.handleError(
			LOC("$$$/LrGeniusAI/People/CollectionSetError=Collection set error"),
			LOC("$$$/LrGeniusAI/People/CollectionSetErrorMessage=Could not create collection set for people.")
		)
		return
	end

	catalog:withWriteAccessDo("Create Collection", function()
		collection = catalog:createCollection(collectionName, collectionSet, false)
	end, Defaults.catalogWriteAccessOptions)
	if not collection then
		ErrorHandler.handleError(
			LOC("$$$/LrGeniusAI/People/CollectionError=Collection error"),
			LOC("$$$/LrGeniusAI/People/CollectionErrorMessage=Could not create collection for this person.")
		)
		return
	end

	catalog:withWriteAccessDo("Add Photos to Collection", function()
		collection:addPhotos(photos)
	end, Defaults.catalogWriteAccessOptions)

	catalog:setActiveSources({ collection })
	LrApplicationView.gridView()
	LrDialogs.message(
		LOC("$$$/LrGeniusAI/People/Done=Done"),
		LOC(
			'$$$/LrGeniusAI/People/CollectionCreated=^1 photo(s) added to collection "^2".',
			tostring(#photos),
			collectionName
		)
	)
end

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("TaskPeople", function(context)
		if not Util.waitForServerDialog() then
			return
		end
		local persons, loadError = loadPersonsFromServer()
		while true do
			local ok, r, pending = LrTasks.pcall(showPeopleDialog, context, persons, loadError)
			if not ok then
				ErrorHandler.handleError(LOC("$$$/LrGeniusAI/People/ErrorTitle=Error"), tostring(r))
				return
			end
			if
				r == "show_library"
				and type(pending) == "table"
				and type(pending.entries) == "table"
				and #pending.entries > 0
			then
				doShowInLibrary(pending.entries, pending.matchMode)
				return
			end
			if r == "reset" then
				-- Dialog erneut öffnen; Namen kommen weiter aus persons (nur nach Save geändert).
			else
				break
			end
		end
	end)
end)
