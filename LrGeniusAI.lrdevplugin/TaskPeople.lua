--[[
    People: list face clusters (persons), assign names, and show photos in Library.
]]

--- Speichert Base64-Thumbnail einer Person in eine Temp-Datei. Setzt person.thumbnail_path.
local function savePersonThumbnail(person, index)
    local thumb = person and person.thumbnail
    if not thumb or thumb == "" then
        person.thumbnail_path = nil
        return
    end
    local tempDir = LrPathUtils.getStandardFilePath('temp')
    local safeId = (person.person_id and person.person_id ~= "") and person.person_id or ("person_" .. tostring(index))
    local safeIdClean = safeId:gsub("[^%w_-]", "_")
    local tempFile = LrPathUtils.child(tempDir, "lrgenius_person_" .. safeIdClean .. ".jpg")
    local f = io.open(tempFile, "wb")
    if f then
        f:write(LrStringUtils.decodeBase64(thumb))
        f:close()
        person.thumbnail_path = tempFile
    else
        person.thumbnail_path = nil
    end
end

--- Schreibt für alle Personen mit Thumbnail-Daten die Temp-Dateien und setzt thumbnail_path.
local function savePersonThumbnails(persons)
    if not persons then return end
    for i, p in ipairs(persons) do
        savePersonThumbnail(p, i)
    end
end

--- Lädt Personen vom Server, speichert Thumbnails in Temp-Dateien. Gibt persons (Tabelle), loadError (string|nil) zurück.
local function loadPersonsFromServer()
    local resp, err = SearchIndexAPI.getPersons()
    if err then
        return {}, (LOC "$$$/LrGeniusAI/People/LoadError=Could not load persons. Check server connection. Try 'Cluster faces' or close and reopen.")
    end
    local persons = (resp and resp.persons) and resp.persons or {}
    savePersonThumbnails(persons)
    return persons, nil
end

local function showSetNameDialog(currentName)
    local resultName
    LrFunctionContext.callWithContext("showSetNameDialog", function(context)
        local f = LrView.osFactory()
        local bind = LrView.bind
        local nameProps = LrBinding.makePropertyTable(context)
        nameProps.name = currentName
        local result = LrDialogs.presentModalDialog {
            title = LOC "$$$/LrGeniusAI/People/SetNameTitle=Set name for person",
            contents = f:column {
                bind_to_object = nameProps,
                f:row {
                    f:static_text { title = LOC "$$$/LrGeniusAI/People/Name=Name:", width = 80 },
                    f:edit_field { value = bind "name", width_in_chars = 25 },
                },
            },
            actionVerb = LOC "$$$/LrGeniusAI/common/Save=Save",
            cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
        }
        if result == "ok" then
            resultName = nameProps.name or ""
        end
    end)
    return resultName
end

--- Zeigt den Personen-Dialog. persons müssen bereits geladen sein (Thumbnails in Temp-Dateien). loadError optional bei Ladefehler.
local function showPeopleDialog(ctx, persons, loadError)
    local f = LrView.osFactory()
    local bind = LrView.bind

    persons = persons or {}

    local props = LrBinding.makePropertyTable(ctx)
    props.persons = persons
    props.selectedPersonIndex = (#persons > 0) and 1 or 0

    -- Zeilen für Thumbnail-Liste: je Person eine Zeile (Radio + Thumbnail + Name/Anzahl)
    local listRows = {}
    if #persons == 0 then
        listRows[1] = f:static_text {
            title = loadError or LOC "$$$/LrGeniusAI/People/NoPersons=No persons yet. Run 'Cluster faces' after indexing photos with face embeddings.",
        }
    else
        for i, p in ipairs(persons) do
            local name = (p.name and p.name ~= "") and p.name or LOC "$$$/LrGeniusAI/People/Unnamed=Unnamed"
            local line = string.format("%s (%d %s, %d %s)",
                name,
                p.face_count or 0,
                (p.face_count or 0) == 1 and (LOC "$$$/LrGeniusAI/People/Face=face") or (LOC "$$$/LrGeniusAI/People/Faces=faces"),
                p.photo_count or 0,
                (p.photo_count or 0) == 1 and (LOC "$$$/LrGeniusAI/People/Photo=photo") or (LOC "$$$/LrGeniusAI/People/Photos=photos"))
            local thumbView = (p.thumbnail_path and p.thumbnail_path ~= "") and f:picture { value = p.thumbnail_path, width = 48, height = 48 } or f:spacer { width = 48, height = 48 }
            listRows[#listRows + 1] = f:row {
                spacing = f:control_spacing(),
                f:radio_button { value = bind "selectedPersonIndex", checked_value = i, title = "" },
                thumbView,
                f:static_text { title = line },
            }
        end
    end

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        fill_horizontal = 1,

        f:row {
            f:push_button {
                title = LOC "$$$/LrGeniusAI/People/ClusterFaces=Cluster faces",
                action = function()
                    local clusterResp, err = SearchIndexAPI.clusterFaces()
                    if err then
                        ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/ClusterError=Face clustering failed", err)
                        return
                    end
                    LrDialogs.message(LOC "$$$/LrGeniusAI/People/ClusterDone=Clustering done",
                        LOC("$$$/LrGeniusAI/People/ClusterSummaryAndReopen=^1 persons, ^2 faces. Close this dialog and open 'People...' again to see the updated list.", tostring(clusterResp and clusterResp.person_count or 0), tostring(clusterResp and clusterResp.face_count or 0)))
                end,
            },
        },

        f:static_text {
            title = LOC "$$$/LrGeniusAI/People/ListTitle=Persons (select one to set name or show in Library):",
            font = "<system/bold>",
        },

        f:scrolled_view {
            horizontal_scroller = false,
            vertical_scroller = true,
            width = 420,
            height = 220,
            f:column {
                unpack(listRows),
            },
        },

        f:row {
            spacing = f:control_spacing(),
            f:push_button {
                title = LOC "$$$/LrGeniusAI/People/SetName=Set name...",
                enabled = bind {
                    key = "selectedPersonIndex",
                    transform = function(value)
                        return value and props.persons and #props.persons > 0 and value >= 1 and value <= #props.persons
                    end,
                },
                action = function()
                    local idx = props.selectedPersonIndex
                    if not props.persons or idx < 1 or idx > #props.persons then return end
                    local person = props.persons[idx]
                    local personId = person.person_id
                    if not personId or personId == "" then return end
                    local currentName = person.name or ""
                    local newName = showSetNameDialog(currentName)
                    if newName ~= nil then
                        local ok, err = SearchIndexAPI.setPersonName(personId, newName)
                        if not ok then
                            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/SetNameError=Could not set name", err)
                            return
                        end
                        -- Name ist gespeichert; beim nächsten Öffnen des Dialogs sichtbar
                    end
                end,
            },
        },
    }

    local dialogResult = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/People/WindowTitle=People",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/common/Close=Close",
        otherVerb = LOC "$$$/LrGeniusAI/People/ShowInLibrary=Show in Library",
    }
    -- Wenn User "Show in Library" (otherVerb) geklickt hat: Auswahl aus props auslesen
    local pendingShowInLibrary = nil
    if dialogResult == "other" then
        local idx = props.selectedPersonIndex
        if type(props.persons) == "table" and idx and idx >= 1 and idx <= #props.persons then
            local person = props.persons[idx]
            if type(person) == "table" and person.person_id and person.person_id ~= "" then
                local name = (type(person.name) == "string" and person.name ~= "") and person.name or nil
                pendingShowInLibrary = { person_id = person.person_id, person_name = name }
            end
        end
    end
    return dialogResult, pendingShowInLibrary
end

--- Führt "Show in Library" aus: Fotos laden, Collection anlegen, Ansicht wechseln. Läuft im Async-Task (Yielding erlaubt).
local function doShowInLibrary(person_id, person_name)
    local resp, err = SearchIndexAPI.getPhotosForPerson(person_id)
    if err or type(resp) ~= "table" then
        ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/GetPhotosError=Could not get photos for person", err or "No data")
        return
    end
    local uuids = type(resp.photo_uuids) == "table" and resp.photo_uuids or {}
    if #uuids == 0 then
        LrDialogs.message(LOC "$$$/LrGeniusAI/People/NoPhotos=No photos", LOC "$$$/LrGeniusAI/People/NoPhotosForPerson=No photos found for this person.")
        return
    end
    local catalog = LrApplication.activeCatalog()
    local photos = {}
    for _, uuid in ipairs(uuids) do
        local photo = catalog:findPhotoByUuid(uuid)
        if photo then table.insert(photos, photo) end
    end
    if #photos == 0 then
        LrDialogs.message(LOC "$$$/LrGeniusAI/People/NoPhotosInCatalog=Not in catalog", LOC "$$$/LrGeniusAI/People/PersonPhotosNotInCatalog=Photos for this person are not in the current catalog.")
        return
    end
    local personDisplayName = (type(person_name) == "string" and person_name ~= "") and person_name or (LOC "$$$/LrGeniusAI/People/Unnamed=Unnamed")
    local collectionName = string.format("%s @ %s", tostring(personDisplayName), LrDate.timeToW3CDate(LrDate.currentTime()))

    local collectionSet, collection
    catalog:withWriteAccessDo("Create Collection Set", function()
        collectionSet = catalog:createCollectionSet(LOC "$$$/LrGeniusAI/People/CollectionSetName=People", nil, true)
    end, Defaults.catalogWriteAccessOptions)
    if not collectionSet then
        ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/CollectionSetError=Collection set error", LOC "$$$/LrGeniusAI/People/CollectionSetErrorMessage=Could not create collection set for people.")
        return
    end

    catalog:withWriteAccessDo("Create Collection", function()
        collection = catalog:createCollection(collectionName, collectionSet, false)
    end, Defaults.catalogWriteAccessOptions)
    if not collection then
        ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/CollectionError=Collection error", LOC "$$$/LrGeniusAI/People/CollectionErrorMessage=Could not create collection for this person.")
        return
    end

    catalog:withWriteAccessDo("Add Photos to Collection", function()
        collection:addPhotos(photos)
    end, Defaults.catalogWriteAccessOptions)

    catalog:setActiveSources({collection})
    LrApplicationView.gridView()
    LrDialogs.message(LOC "$$$/LrGeniusAI/People/Done=Done", LOC("$$$/LrGeniusAI/People/CollectionCreated=^1 photo(s) added to collection \"^2\".", tostring(#photos), collectionName))
end

LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext("TaskPeople", function(context)
        if not Util.waitForServerDialog() then return end
        local persons, loadError = loadPersonsFromServer()
        local ok, result, pending = pcall(showPeopleDialog, context, persons, loadError)
        if not ok then
            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/People/ErrorTitle=Error", tostring(result))
            return
        end
        -- pending wird gesetzt wenn User "Show in Library" geklickt hat (Dialog schließt mit "ok")
        if pending and type(pending) == "table" and pending.person_id then
            doShowInLibrary(pending.person_id, pending.person_name)
        end
    end)
end)
