-- TaskRetrieveMetadata.lua
-- Retrieves stored metadata from the backend and writes them to the Lightroom catalog.
-- Allows user to select which data fields to transfer and optionally validate before applying.


---
-- Shows the configuration dialog for metadata retrieval.
-- @param ctx The LrFunctionContext for the dialog.
-- @return table with configuration options or nil if canceled.
--
local function showRetrieveMetadataDialog(ctx)
    local f = LrView.osFactory()
    local bind = LrView.bind
    local share = LrView.share

    local props = LrBinding.makePropertyTable(ctx)
    
    -- Scope settings
    props.scope = prefs.retrieveMetadataScope or "selected"
    
    -- Data fields to retrieve and apply
    props.applyKeywords = prefs.applyKeywords ~= false -- default true
    props.applyTitle = prefs.applyTitle ~= false -- default true
    props.applyCaption = prefs.applyCaption ~= false -- default true
    props.applyAltText = prefs.applyAltText ~= false -- default true
    props.useTopLevelKeyword = prefs.useTopLevelKeyword ~= false -- default true
    props.topLevelKeyword = prefs.topLevelKeyword or "LrGeniusAI"
    
    -- Validation option
    props.enableValidation = prefs.retrieveEnableValidation or false
    props.appendMetadata = prefs.retrieveAppendMetadata or false

    local scopeItems = {
        { title = LOC "$$$/LrGeniusAI/Scope/Selected=Selected Photos", value = 'selected' },
        { title = LOC "$$$/LrGeniusAI/Scope/View=Current View", value = 'view' },
        { title = LOC "$$$/LrGeniusAI/Scope/All=All Photos", value = 'all' },
    }

    local dialogView = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        
        -- Scope Selection
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Scope=Photo Selection",
            fill_horizontal = 1,
            f:row {
                f:static_text {
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/ScopeLabel=Apply to:",
                    width = share 'labelWidth',
                },
                f:popup_menu {
                    value = bind 'scope',
                    items = scopeItems,
                    fill_horizontal = 1,
                },
            },
        },
        
        -- Data Fields to Apply
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/DataFields=Data Fields to Apply",
            fill_horizontal = 1,
            f:column {
                spacing = f:control_spacing(),
                f:checkbox {
                    value = bind 'applyKeywords',
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Keywords=Keywords",
                },
                f:checkbox {
                    value = bind 'applyTitle',
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Title=Title",
                },
                f:checkbox {
                    value = bind 'applyCaption',
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Caption=Caption",
                },
                f:checkbox {
                    value = bind 'applyAltText',
                    title = LOC "$$$/LrGeniusAI/RetrieveMetadata/AltText=Alt Text",
                },
            },
        },
        
        -- Validation Option
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/Validation=Validation",
            fill_horizontal = 1,
            f:checkbox {
                value = bind 'enableValidation',
                title = LOC "$$$/LrGeniusAI/RetrieveMetadata/EnableValidation=Review data before applying to catalog",
            },
        },

        -- Append metadata option
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/AppendOption=Apply Options",
            fill_horizontal = 1,
            f:checkbox {
                value = bind 'appendMetadata',
                title = LOC "$$$/LrGeniusAI/RetrieveMetadata/AppendMetadata=Append metadata (do not overwrite existing)",
            },
        },

        -- Keyword Option
        f:group_box {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/UseTopLevelKeyword=Use top-level keyword for applied keywords",
            fill_horizontal = 1,
            f:row {
                f:checkbox {
                    value = bind 'useTopLevelKeyword',
                },
                f:edit_field {
                    value = bind 'topLevelKeyword',
                    width_in_chars = 20,
                    enabled = bind 'useTopLevelKeyword',
                },
            },
        },
    }

    local result = LrDialogs.presentModalDialog {
        title = LOC "$$$/LrGeniusAI/RetrieveMetadata/DialogTitle=Retrieve Metadata from Backend",
        contents = dialogView,
        actionVerb = LOC "$$$/LrGeniusAI/RetrieveMetadata/ActionVerb=Retrieve",
    }

    if result == 'ok' then
        -- Save preferences
        prefs.retrieveMetadataScope = props.scope
        prefs.applyKeywords = props.applyKeywords
        prefs.applyTitle = props.applyTitle
        prefs.applyCaption = props.applyCaption
        prefs.applyAltText = props.applyAltText
        prefs.retrieveEnableValidation = props.enableValidation
        prefs.retrieveAppendMetadata = props.appendMetadata
        prefs.useTopLevelKeyword = props.useTopLevelKeyword
        prefs.topLevelKeyword = props.topLevelKeyword
        
        return {
            scope = props.scope,
            applyKeywords = props.applyKeywords,
            applyTitle = props.applyTitle,
            applyCaption = props.applyCaption,
            applyAltText = props.applyAltText,
            enableValidation = props.enableValidation,
            appendMetadata = props.appendMetadata,
            useTopLevelKeyword = props.useTopLevelKeyword,
            topLevelKeyword = props.topLevelKeyword,
        }
    else
        return nil
    end
end


---
-- Main task function.
--
LrTasks.startAsyncTask(function()
    LrFunctionContext.callWithContext('retrieveMetadataTask', function(ctx)

        -- Check server connection
        if not Util.waitForServerDialog() then return end


        -- Show configuration dialog
        local options = showRetrieveMetadataDialog(ctx)
        if not options then
            log:info("Retrieve metadata task canceled by user")
            return
        end

        -- Get photos based on scope
        local photos, status = PhotoSelector.getPhotosInScope(options.scope)
        if not photos or #photos == 0 then
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/RetrieveMetadata/NoPhotos=No Photos",
                LOC "$$$/LrGeniusAI/RetrieveMetadata/NoPhotosMessage=No photos found in the selected scope.",
                "info"
            )
            return
        end

        log:info(string.format("Starting metadata retrieval for %d photos", #photos))

        -- Progress indicator
        local progressScope = LrProgressScope {
            title = LOC "$$$/LrGeniusAI/RetrieveMetadata/ProgressTitle=Retrieving Metadata from Backend",
        }

        local successCount = 0
        local skipCount = 0
        local errorCount = 0
        local errorMessages = {}
        local backendWarnings = {}
        local skipValidation = false

        for i, photo in ipairs(photos) do
            if progressScope:isCanceled() then
                log:info("Retrieve metadata task canceled by user")
                break
            end

            local fileName = photo:getFormattedMetadata('fileName')
            progressScope:setPortionComplete(i - 1, #photos)
            progressScope:setCaption(string.format(LOC "$$$/LrGeniusAI/RetrieveMetadata/Processing=Processing %s (%d of %d)", 
                fileName, i, #photos))

            local photoId, photoIdErr = SearchIndexAPI.getPhotoIdForPhoto(photo)
            if photoId then
                if not SearchIndexAPI.pingServer() then
                    LrDialogs.message(
                        LOC "$$$/LrGeniusAI/RetrieveMetadata/ServerUnreachable=Server Unreachable",
                        LOC "$$$/LrGeniusAI/RetrieveMetadata/ServerUnreachableMessage=Cannot reach backend server. Please check your connection and try again.",
                        "error"
                    )
                    log:error("Backend server unreachable during metadata retrieval")
                    break
                end
                -- Retrieve data from backend
                log:trace("Retrieving data for photo_id: " .. photoId)
                local retrievedData, err = SearchIndexAPI.getPhotoData(photoId)

                log:trace("Retrieved data: " .. Util.dumpTable(retrievedData))
                
                if err then
                    log:error("Error retrieving metadata for " .. fileName .. ": " .. tostring(err))
                    table.insert(errorMessages, fileName .. ": " .. tostring(err))
                    errorCount = errorCount + 1
                elseif retrievedData and retrievedData.status == "success" then
                    if retrievedData.warning then
                        table.insert(backendWarnings, fileName .. ": " .. tostring(retrievedData.warning))
                    end
                    -- Validate if requested and not skipped
                    local shouldApply = true
                    local validatedData = nil
                    local result = nil
                    if options.enableValidation and not skipValidation then
                        result, validatedData = MetadataManager.showValidationDialog(ctx, photo, retrievedData, options)
                        
                        if result == "ok" then
                            if validatedData ~= nil and validatedData.skipFromHere then
                                skipValidation = true
                            end
                        elseif result == "other" then
                            skipCount = skipCount + 1
                            shouldApply = false
                            validatedData = nil
                            -- Clear only metadata so the photo stays in the index and can be regenerated later
                            SearchIndexAPI.removePhotoMetadata(photoId)
                            Util.addPhotoToRejectedDescriptionsCollection(photo, Defaults.catalogWriteAccessOptions)
                        else
                            -- Validation canceled
                            break
                        end
                    end

                    -- Apply metadata
                    if shouldApply then
                        MetadataManager.applyMetadata(photo, retrievedData, validatedData, options)
                        successCount = successCount + 1
                        log:trace("Metadata applied successfully for photo: " .. fileName)
                        
                        -- Overwrite with validated data if any
                        if result ~= nil and result == "ok" then
                            SearchIndexAPI.importMetadataFromCatalog({ photo }, progressScope, false)
                        end
                    end
                else
                    log:warn("No data found in backend for photo: " .. fileName)
                    table.insert(errorMessages, fileName .. ": " .. LOC "$$$/LrGeniusAI/RetrieveMetadata/ErrorNoData=No data found")
                    errorCount = errorCount + 1
                end
            else
                log:warn("Photo has no usable photo_id, skipping: " .. fileName .. " (" .. tostring(photoIdErr) .. ")")
                table.insert(errorMessages, fileName .. ": " .. tostring(photoIdErr))
                errorCount = errorCount + 1
            end
        end

        progressScope:done()

        if errorCount > 0 or #backendWarnings > 0 then
            local uniqueErrors = {}
            local errorList = {}
            for _, msg in ipairs(errorMessages) do
                if not uniqueErrors[msg] then
                    uniqueErrors[msg] = true
                    table.insert(errorList, "- " .. msg)
                    if #errorList >= 5 then break end
                end
            end

            local combinedReport = LOC("$$$/LrGeniusAI/RetrieveMetadata/Summary=Retrieved metadata for ^1 photo(s).", tostring(successCount))
            if skipCount > 0 then
                combinedReport = combinedReport .. "\n" .. LOC("$$$/LrGeniusAI/common/Skipped=Skipped: ^1", tostring(skipCount))
            end
            if errorCount > 0 then
                combinedReport = combinedReport .. "\n" .. LOC("$$$/LrGeniusAI/common/Errors=Errors: ^1", tostring(errorCount))
            end

            if #errorList > 0 then
                combinedReport = combinedReport .. "\n\n" .. LOC "$$$/LrGeniusAI/common/ErrorDetails=Error details:" .. "\n" .. table.concat(errorList, "\n")
                if #errorMessages > 5 then
                    combinedReport = combinedReport .. "\n" .. LOC("$$$/LrGeniusAI/common/MoreErrors=... and ^1 more errors", tostring(#errorMessages - 5))
                end
            end

            if #backendWarnings > 0 then
                combinedReport = combinedReport .. "\n\n" .. LOC "$$$/LrGeniusAI/common/BackendWarnings=Backend Warnings:" .. "\n"
                for i = 1, math.min(5, #backendWarnings) do
                    combinedReport = combinedReport .. "- " .. backendWarnings[i] .. "\n"
                end
                if #backendWarnings > 5 then
                    combinedReport = combinedReport .. LOC("$$$/LrGeniusAI/common/MoreWarnings=... and ^1 more warnings", tostring(#backendWarnings - 5))
                end
            end

            ErrorHandler.handleError(LOC "$$$/LrGeniusAI/RetrieveMetadata/CompletionTitle=Metadata Retrieval Completed", combinedReport)
        else
            LrDialogs.message(
                LOC "$$$/LrGeniusAI/RetrieveMetadata/SuccessTitle=Metadata Retrieval",
                LOC("$$$/LrGeniusAI/RetrieveMetadata/SuccessSummary=Successfully retrieved metadata for ^1 photo(s).\nSkipped: ^2", tostring(successCount), tostring(skipCount)),
                "info"
            )
        end

        log:info(string.format("Retrieve metadata task complete. Success: %d, Skipped: %d, Errors: %d", 
            successCount, skipCount, errorCount))
    end)
end)
