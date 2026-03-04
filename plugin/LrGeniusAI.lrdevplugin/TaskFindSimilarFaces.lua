--[[
    Find Similar Faces: Select a face on the current photo, get its cluster, create a collection.
    1. Dialog shows detected faces on the photo
    2. User selects one face
    3. Fetch face cluster (person) from server
    4. Create collection and show in Library (same as People "Show in Library")
]]

--- Save face thumbnail (base64) to temp file. Sets face.thumbnail_path.
local function saveFaceThumbnail(face, index)
    local thumb = face and face.thumbnail
    if not thumb or thumb == "" then
        face.thumbnail_path = nil
        return
    end
    local tempDir = LrPathUtils.getStandardFilePath('temp')
    local tempFile = LrPathUtils.child(tempDir, "lrgenius_face_" .. tostring(index) .. ".jpg")
    local f = io.open(tempFile, "wb")
    if f then
        f:write(LrStringUtils.decodeBase64(thumb))
        f:close()
        face.thumbnail_path = tempFile
    else
        face.thumbnail_path = nil
    end
end

local function saveFaceThumbnails(faces)
    if not faces then return end
    for i, face in ipairs(faces) do
        saveFaceThumbnail(face, i)
    end
end

--- Resolve person name from person_id using persons list. Returns display name or nil.
local function getPersonNameForId(personId, persons)
    if not personId or personId == "" or not persons then return nil end
    for _, p in ipairs(persons) do
        if p.person_id == personId then
            local name = (p.name and p.name ~= "") and p.name or nil
            return name
        end
    end
    return nil
end

--- Show dialog with detected faces. Returns selected face index (1-based) or nil if canceled.
local function showFaceSelectionDialog(context, faces)
    local f = LrView.osFactory()
    local bind = LrView.bind

    local props = LrBinding.makePropertyTable(context)
    props.faces = faces
    props.selectedFaceIndex = (#faces > 0) and 1 or 0

    local listRows = {}
    for i, face in ipairs(faces) do
        local label
        if face.person_name and face.person_name ~= "" then
            label = LOC("$$$/LrGeniusAI/FindSimilarFaces/FaceWithName=Face ^1 (^2)", tostring(i), face.person_name)
        else
            label = LOC("$$$/LrGeniusAI/FindSimilarFaces/FaceNumber=Face ^1", tostring(i))
        end
        local thumbView = (face.thumbnail_path and face.thumbnail_path ~= "") and f:picture { value = face.thumbnail_path, width = 48, height = 48 } or f:spacer { width = 48, height = 48 }
        listRows[#listRows + 1] = f:row {
            spacing = f:control_spacing(),
            f:radio_button { value = bind "selectedFaceIndex", checked_value = i, title = "" },
            thumbView,
            f:static_text { title = label },
        }
    end

    local listScroller = f:scrolled_view {
        horizontal_scroller = false,
        vertical_scroller = true,
        width = 320,
        height = 200,
        f:column { unpack(listRows) },
    }

    local contents = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        fill_horizontal = 1,
        f:static_text {
            title = LOC "$$$/LrGeniusAI/FindSimilarFaces/SelectFace=Select the face to search for:",
            font = "<system/bold>",
        },
        listScroller,
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/FindSimilarFaces/WindowTitle=Find Similar Faces",
        contents = contents,
        actionVerb = LOC "$$$/LrGeniusAI/FindSimilarFaces/Search=Search",
        cancelVerb = LOC "$$$/LrGeniusAI/common/Cancel=Cancel",
    }
    if result == "cancel" then return nil end
    return props.selectedFaceIndex
end

--- Create collection from photo UUIDs and show in Library (same pattern as TaskPeople doShowInLibrary).
local function createCollectionFromUuids(uuids, collectionName)
    local catalog = LrApplication.activeCatalog()
    local photos = {}
    for _, uuid in ipairs(uuids) do
        local photo = catalog:findPhotoByUuid(uuid)
        if photo then table.insert(photos, photo) end
    end
    if #photos == 0 then
        LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/NoPhotosInCatalog=Not in catalog", LOC "$$$/LrGeniusAI/FindSimilarFaces/PersonPhotosNotInCatalog=Photos for this person are not in the current catalog.")
        return
    end

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
    LrFunctionContext.callWithContext("TaskFindSimilarFaces", function(context)
        if not Util.waitForServerDialog() then return end

        local catalog = LrApplication.activeCatalog()
        local targetPhotos = catalog:getTargetPhotos()
        if not targetPhotos or #targetPhotos == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/NoPhotoTitle=No photo selected", LOC "$$$/LrGeniusAI/FindSimilarFaces/NoPhotoMessage=Please select a single photo in the Library.")
            return
        end
        if #targetPhotos > 1 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/SinglePhotoTitle=Select one photo", LOC "$$$/LrGeniusAI/FindSimilarFaces/SinglePhotoMessage=Please select exactly one photo to find similar faces.")
            return
        end

        local photo = targetPhotos[1]
        local path = SearchIndexAPI.exportPhotoForIndexing(photo)
        if not path or not LrFileUtils.exists(path) then
            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/FindSimilarFaces/ExportError=Export failed", "Could not export photo for face detection.")
            return
        end

        local imageBase64 = Util.encodePhotoToBase64(path)
        LrFileUtils.delete(path)
        if not imageBase64 then
            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/FindSimilarFaces/ExportError=Export failed", "Could not read exported image.")
            return
        end

        -- 1) Detect faces
        local detectResp, err = SearchIndexAPI.detectFacesInImage(imageBase64)
        if err then
            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/FindSimilarFaces/DetectError=Face detection failed", err)
            return
        end
        local faces = (detectResp and detectResp.faces) and detectResp.faces or {}
        if #faces == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/NoFacesTitle=No faces found", LOC "$$$/LrGeniusAI/FindSimilarFaces/NoFacesMessage=No faces were detected on this photo.")
            return
        end

        saveFaceThumbnails(faces)

        -- Resolve person names for each face (quick query per face, then look up name)
        local personsResp, _ = SearchIndexAPI.getPersons()
        local persons = (personsResp and personsResp.persons) and personsResp.persons or {}
        for i, face in ipairs(faces) do
            local qResp, _ = SearchIndexAPI.queryFacesByImage(imageBase64, i - 1, 1)
            local qResults = (qResp and qResp.results) and qResp.results or {}
            if qResults[1] and qResults[1].person_id then
                local name = getPersonNameForId(qResults[1].person_id, persons)
                if name then face.person_name = name end
            end
        end

        -- 2) User selects face (1-based index)
        local selectedIndex = showFaceSelectionDialog(context, faces)
        if not selectedIndex or selectedIndex < 1 or selectedIndex > #faces then return end

        -- 3) Query similar faces (face_index is 0-based)
        local faceIndex = selectedIndex - 1
        local queryResp, err = SearchIndexAPI.queryFacesByImage(imageBase64, faceIndex, 500)
        if err then
            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/FindSimilarFaces/QueryError=Search failed", err)
            return
        end
        local results = (queryResp and queryResp.results) and queryResp.results or {}
        if #results == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/NoResultsTitle=No similar faces", LOC "$$$/LrGeniusAI/FindSimilarFaces/NoResultsMessage=No similar faces found in the index. Run 'Analyze & Index' with face detection enabled.")
            return
        end

        -- 4) Get cluster: prefer person_id from first result, then get all photos for that person
        local personId = results[1].person_id
        local uuids = {}
        if personId and personId ~= "" then
            local personResp, personErr = SearchIndexAPI.getPhotosForPerson(personId)
            if not personErr and personResp and personResp.photo_uuids then
                uuids = personResp.photo_uuids
            end
        end
        if #uuids == 0 then
            local seen = {}
            for _, r in ipairs(results) do
                local uuid = r.photo_uuid
                if uuid and not seen[uuid] then
                    seen[uuid] = true
                    table.insert(uuids, uuid)
                end
            end
        end

        if #uuids == 0 then
            LrDialogs.message(LOC "$$$/LrGeniusAI/FindSimilarFaces/NoResultsTitle=No similar faces", LOC "$$$/LrGeniusAI/FindSimilarFaces/NoPhotosForFace=No photos found for this face.")
            return
        end

        -- Collection name: use person name when available (same as People "Show in Library")
        local personDisplayName
        if personId and personId ~= "" then
            local personsResp2, _ = SearchIndexAPI.getPersons()
            local personsList = (personsResp2 and personsResp2.persons) and personsResp2.persons or {}
            personDisplayName = getPersonNameForId(personId, personsList)
        end
        if not personDisplayName or personDisplayName == "" then
            personDisplayName = LOC "$$$/LrGeniusAI/FindSimilarFaces/SimilarFaces=Similar Faces"
        end
        local collectionName = string.format("%s @ %s", personDisplayName, LrDate.timeToW3CDate(LrDate.currentTime()))
        createCollectionFromUuids(uuids, collectionName)
    end)
end)
