PhotoSelector = {}

local function filterPhotos(photos)
    if not photos then return {} end
    local filteredPhotos = {}
    for _, photo in ipairs(photos) do
        if not photo:getRawMetadata("isVideo") then
            table.insert(filteredPhotos, photo)
        end
    end
    return filteredPhotos
end

---
-- @param scope string 'selected'|'view'|'all'|'missing'
-- @param taskOptions table|boolean|nil For scope 'missing': task options table
--   { enableEmbeddings, enableMetadata, enableQuality, enableFaces, regenerateMetadata }
--   to check backend for unprocessed photos. Or boolean for legacy (requireEmbeddings).
--   Nil/omitted = legacy true (photos not in index with embeddings).
--
function PhotoSelector.getPhotosInScope(scope, taskOptions)
    local catalog = LrApplication.activeCatalog()
    local photosToProcess = {}
    local status = "ok"

    if scope == 'selected' then
        photosToProcess = filterPhotos(catalog:getTargetPhotos())
    elseif scope == 'view' then
        local sources = catalog:getActiveSources()
        if not sources or #sources == 0 then
            return nil, "No active source"
        end
        local addedPhotos = {}
        local containsAllPhotos = false

        for _, source in ipairs(sources) do
            if type(source) == "string" then
                if source == "kAllPhotos" then
                    photosToProcess = filterPhotos(catalog:getAllPhotos())
                    containsAllPhotos = true
                    break -- No need to process other sources
                elseif source == "kPreviousImport" then
                    local previousImport = filterPhotos(catalog:getPreviousImport())
                    if previousImport then
                        for _, photo in ipairs(previousImport) do
                            local photoId = photo:getRawMetadata("uuid")
                            if not addedPhotos[photoId] then
                                table.insert(photosToProcess, photo)
                                addedPhotos[photoId] = true
                            end
                        end
                    end
                else
                    log:warn("Unsupported string source type: " .. source)
                end
            elseif source and (source:type() == 'LrCollection' or source:type() == 'LrFolder' or source:type() == 'LrCollectionSet' or source:type() == 'LrPublishedCollection' or source:type() == 'LrPublishedCollectionSet') then
                local photos = filterPhotos(source:getPhotos())
                for _, photo in ipairs(photos) do
                    local photoId = photo:getRawMetadata("uuid")
                    if not addedPhotos[photoId] then
                        table.insert(photosToProcess, photo)
                        addedPhotos[photoId] = true
                    end
                end
            else
                if source and source.type then
                    log:warn("Unsupported source type for grouping similar photos: " .. source:type())
                else
                    log:warn("Unsupported source type for grouping similar photos: " .. type(source))
                end
            end
        end
        if #photosToProcess == 0 then
            return nil, "Invalid view"
        end
    elseif scope == 'all' then
        photosToProcess = filterPhotos(catalog:getAllPhotos())
    elseif scope == "missing" then
        local success = false
        success, photosToProcess = SearchIndexAPI.getMissingPhotosFromIndex(taskOptions)
        status = success and "ok" or "indexerror"
    end

    return photosToProcess, status
end

return PhotoSelector