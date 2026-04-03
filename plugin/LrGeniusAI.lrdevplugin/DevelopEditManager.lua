DevelopEditManager = {}

local GLOBAL_KEY_MAP = {
    exposure = "Exposure2012",
    contrast = "Contrast2012",
    highlights = "Highlights2012",
    shadows = "Shadows2012",
    whites = "Whites2012",
    blacks = "Blacks2012",
    temperature = "Temp",
    tint = "Tint",
    texture = "Texture",
    clarity = "Clarity2012",
    dehaze = "Dehaze",
    vibrance = "Vibrance",
    saturation = "Saturation",
    sharpening = "Sharpness",
    sharpen_radius = "SharpenRadius",
    sharpen_detail = "SharpenDetail",
    sharpen_masking = "SharpenEdgeMasking",
    noise_reduction = "LuminanceSmoothing",
    noise_reduction_detail = "LuminanceNoiseReductionDetail",
    noise_reduction_contrast = "LuminanceNoiseReductionContrast",
    color_noise_reduction = "ColorNoiseReduction",
    color_noise_reduction_detail = "ColorNoiseReductionDetail",
    color_noise_reduction_smoothness = "ColorNoiseReductionSmoothness",
    vignette = "PostCropVignetteAmount",
    vignette_midpoint = "PostCropVignetteMidpoint",
    vignette_roundness = "PostCropVignetteRoundness",
    vignette_feather = "PostCropVignetteFeather",
    vignette_highlights = "PostCropVignetteHighlightContrast",
    grain = "GrainAmount",
    grain_size = "GrainSize",
    grain_roughness = "GrainFrequency",
}

local MASK_KEY_CANDIDATES = {
    exposure = { "local_Exposure", "Exposure2012", "Exposure" },
    contrast = { "local_Contrast", "Contrast2012", "Contrast" },
    highlights = { "local_Highlights", "Highlights2012", "Highlights" },
    shadows = { "local_Shadows", "Shadows2012", "Shadows" },
    whites = { "local_Whites", "Whites2012", "Whites" },
    blacks = { "local_Blacks", "Blacks2012", "Blacks" },
    temperature = { "local_Temperature", "Temperature", "Temp" },
    tint = { "local_Tint", "Tint" },
    texture = { "local_Texture", "Texture" },
    clarity = { "local_Clarity", "Clarity2012", "Clarity" },
    dehaze = { "local_Dehaze", "Dehaze" },
    saturation = { "local_Saturation", "Saturation" },
    sharpness = { "local_Sharpness", "Sharpness" },
    noise = { "local_Noise", "LuminanceSmoothing" },
    moire = { "local_Moire" },
}

local AI_MASK_TOOL_CANDIDATES = {
    subject = { "subject", "selectSubject", "person" },
    sky = { "sky", "selectSky" },
    people = { "people", "person" },
    person = { "person", "people" },
    object = { "object", "objects" },
    objects = { "objects", "object" },
    background = { "background", "subject", "selectSubject" },
}

local HSL_LABELS = {
    red = "Red",
    orange = "Orange",
    yellow = "Yellow",
    green = "Green",
    aqua = "Aqua",
    blue = "Blue",
    purple = "Purple",
    magenta = "Magenta",
}

local ADDITIVE_GLOBAL_KEYS = {
    Exposure2012 = true,
    Contrast2012 = true,
    Highlights2012 = true,
    Shadows2012 = true,
    Whites2012 = true,
    Blacks2012 = true,
    Temp = true,
    Tint = true,
    Texture = true,
    Clarity2012 = true,
    Dehaze = true,
    Vibrance = true,
    Saturation = true,
    Sharpness = true,
    SharpenRadius = true,
    SharpenDetail = true,
    SharpenEdgeMasking = true,
    LuminanceSmoothing = true,
    LuminanceNoiseReductionDetail = true,
    LuminanceNoiseReductionContrast = true,
    ColorNoiseReduction = true,
    ColorNoiseReductionDetail = true,
    ColorNoiseReductionSmoothness = true,
    PostCropVignetteAmount = true,
    PostCropVignetteMidpoint = true,
    PostCropVignetteRoundness = true,
    PostCropVignetteFeather = true,
    PostCropVignetteHighlightContrast = true,
    GrainAmount = true,
    GrainSize = true,
    GrainFrequency = true,
    SplitToningShadowHue = true,
    SplitToningShadowSaturation = true,
    SplitToningHighlightHue = true,
    SplitToningHighlightSaturation = true,
    SplitToningBalance = true,
    ParametricHighlights = true,
    ParametricLights = true,
    ParametricDarks = true,
    ParametricShadows = true,
}

local DEVELOP_VALUE_BOUNDS = {
    Exposure2012 = { min = -5, max = 5 },
    Contrast2012 = { min = -100, max = 100 },
    Highlights2012 = { min = -100, max = 100 },
    Shadows2012 = { min = -100, max = 100 },
    Whites2012 = { min = -100, max = 100 },
    Blacks2012 = { min = -100, max = 100 },
    Temp = { min = 2000, max = 50000 },
    Tint = { min = -150, max = 150 },
    Texture = { min = -100, max = 100 },
    Clarity2012 = { min = -100, max = 100 },
    Dehaze = { min = -100, max = 100 },
    Vibrance = { min = -100, max = 100 },
    Saturation = { min = -100, max = 100 },
    Sharpness = { min = 0, max = 150 },
    SharpenRadius = { min = 0.5, max = 3.0 },
    SharpenDetail = { min = 0, max = 100 },
    SharpenEdgeMasking = { min = 0, max = 100 },
    LuminanceSmoothing = { min = 0, max = 100 },
    LuminanceNoiseReductionDetail = { min = 0, max = 100 },
    LuminanceNoiseReductionContrast = { min = 0, max = 100 },
    ColorNoiseReduction = { min = 0, max = 100 },
    ColorNoiseReductionDetail = { min = 0, max = 100 },
    ColorNoiseReductionSmoothness = { min = 0, max = 100 },
    PostCropVignetteAmount = { min = -100, max = 100 },
    PostCropVignetteMidpoint = { min = 0, max = 100 },
    PostCropVignetteRoundness = { min = -100, max = 100 },
    PostCropVignetteFeather = { min = 0, max = 100 },
    PostCropVignetteHighlightContrast = { min = 0, max = 100 },
    GrainAmount = { min = 0, max = 100 },
    GrainSize = { min = 0, max = 100 },
    GrainFrequency = { min = 0, max = 100 },
    SplitToningShadowHue = { min = 0, max = 360, wrap = true },
    SplitToningShadowSaturation = { min = 0, max = 100 },
    SplitToningHighlightHue = { min = 0, max = 360, wrap = true },
    SplitToningHighlightSaturation = { min = 0, max = 100 },
    SplitToningBalance = { min = -100, max = 100 },
    ParametricHighlights = { min = -100, max = 100 },
    ParametricLights = { min = -100, max = 100 },
    ParametricDarks = { min = -100, max = 100 },
    ParametricShadows = { min = -100, max = 100 },
    ParametricShadowSplit = { min = 0, max = 100 },
    ParametricMidtoneSplit = { min = 0, max = 100 },
    ParametricHighlightSplit = { min = 0, max = 100 },
}

for _, label in pairs(HSL_LABELS) do
    DEVELOP_VALUE_BOUNDS["HueAdjustment" .. label] = { min = -100, max = 100 }
    DEVELOP_VALUE_BOUNDS["SaturationAdjustment" .. label] = { min = -100, max = 100 }
    DEVELOP_VALUE_BOUNDS["LuminanceAdjustment" .. label] = { min = -100, max = 100 }
    ADDITIVE_GLOBAL_KEYS["HueAdjustment" .. label] = true
    ADDITIVE_GLOBAL_KEYS["SaturationAdjustment" .. label] = true
    ADDITIVE_GLOBAL_KEYS["LuminanceAdjustment" .. label] = true
end

local function appendWarning(warnings, text)
    if warnings and text and text ~= "" then
        table.insert(warnings, text)
    end
end

local function sortedKeys(tbl)
    local keys = {}
    for key in pairs(tbl or {}) do
        table.insert(keys, key)
    end
    table.sort(keys)
    return keys
end

local function tableCount(tbl)
    local count = 0
    for _ in pairs(tbl or {}) do
        count = count + 1
    end
    return count
end

local function getRecipeFromResponse(response)
    if type(response) ~= "table" then
        return nil
    end
    if type(response.edit) == "table" then
        return response.edit
    end
    if type(response.recipe) == "table" then
        return response.recipe
    end
    if type(response.global) == "table" or type(response.masks) == "table" then
        return response
    end
    return nil
end

local function buildHslDevelopSettings(hsl)
    local settings = {}
    if type(hsl) ~= "table" then
        return settings
    end

    for channel, adjustments in pairs(hsl) do
        local label = HSL_LABELS[channel]
        if label and type(adjustments) == "table" then
            if adjustments.hue ~= nil then
                settings["HueAdjustment" .. label] = adjustments.hue
            end
            if adjustments.saturation ~= nil then
                settings["SaturationAdjustment" .. label] = adjustments.saturation
            end
            if adjustments.luminance ~= nil then
                settings["LuminanceAdjustment" .. label] = adjustments.luminance
            end
        end
    end
    return settings
end

local function buildColorGradingDevelopSettings(colorGrading, warnings)
    local settings = {}
    if type(colorGrading) ~= "table" then
        return settings
    end

    local shadows = colorGrading.shadows
    if type(shadows) == "table" then
        if shadows.hue ~= nil then settings.SplitToningShadowHue = shadows.hue end
        if shadows.saturation ~= nil then settings.SplitToningShadowSaturation = shadows.saturation end
        if shadows.luminance ~= nil then
            appendWarning(warnings, "Shadow color grading luminance is not supported by Lightroom develop settings and was ignored.")
        end
    end

    local highlights = colorGrading.highlights
    if type(highlights) == "table" then
        if highlights.hue ~= nil then settings.SplitToningHighlightHue = highlights.hue end
        if highlights.saturation ~= nil then settings.SplitToningHighlightSaturation = highlights.saturation end
        if highlights.luminance ~= nil then
            appendWarning(warnings, "Highlight color grading luminance is not supported by Lightroom develop settings and was ignored.")
        end
    end

    if colorGrading.balance ~= nil then
        settings.SplitToningBalance = colorGrading.balance
    end
    if type(colorGrading.midtones) == "table" then
        appendWarning(warnings, "Midtone color grading is not currently mapped by the Lightroom plugin and was ignored.")
    end
    if type(colorGrading.global) == "table" then
        appendWarning(warnings, "Global color grading is not currently mapped by the Lightroom plugin and was ignored.")
    end
    if colorGrading.blending ~= nil then
        appendWarning(warnings, "Color grading blending is not currently mapped by the Lightroom plugin and was ignored.")
    end

    if next(settings) ~= nil then
        settings.EnableSplitToning = true
    end
    return settings
end

local function buildToneCurveSettings(toneCurve)
    local settings = {}
    if type(toneCurve) ~= "table" then
        return settings
    end

    if toneCurve.highlights ~= nil then settings.ParametricHighlights = toneCurve.highlights end
    if toneCurve.lights ~= nil then settings.ParametricLights = toneCurve.lights end
    if toneCurve.darks ~= nil then settings.ParametricDarks = toneCurve.darks end
    if toneCurve.shadows ~= nil then settings.ParametricShadows = toneCurve.shadows end
    if toneCurve.shadow_split ~= nil then settings.ParametricShadowSplit = toneCurve.shadow_split end
    if toneCurve.midtone_split ~= nil then settings.ParametricMidtoneSplit = toneCurve.midtone_split end
    if toneCurve.highlight_split ~= nil then settings.ParametricHighlightSplit = toneCurve.highlight_split end

    local pointCurve = toneCurve.point_curve
    local extendedPointCurve = toneCurve.extended_point_curve
    if type(pointCurve) == "table" then
        if type(pointCurve.master) == "table" and #pointCurve.master >= 4 then
            settings.ToneCurvePV2012 = pointCurve.master
        end
        if type(pointCurve.red) == "table" and #pointCurve.red >= 4 then
            settings.ToneCurvePV2012Red = pointCurve.red
        end
        if type(pointCurve.green) == "table" and #pointCurve.green >= 4 then
            settings.ToneCurvePV2012Green = pointCurve.green
        end
        if type(pointCurve.blue) == "table" and #pointCurve.blue >= 4 then
            settings.ToneCurvePV2012Blue = pointCurve.blue
        end
    end

    if type(extendedPointCurve) == "table" then
        if type(extendedPointCurve.master) == "table" and #extendedPointCurve.master >= 4 then
            settings.ExtendedToneCurvePV2012 = extendedPointCurve.master
        end
        if type(extendedPointCurve.red) == "table" and #extendedPointCurve.red >= 4 then
            settings.ExtendedToneCurvePV2012Red = extendedPointCurve.red
        end
        if type(extendedPointCurve.green) == "table" and #extendedPointCurve.green >= 4 then
            settings.ExtendedToneCurvePV2012Green = extendedPointCurve.green
        end
        if type(extendedPointCurve.blue) == "table" and #extendedPointCurve.blue >= 4 then
            settings.ExtendedToneCurvePV2012Blue = extendedPointCurve.blue
        end
    end

    -- Lightroom versions differ in which curve keys they honor on apply.
    -- Provide both standard and extended PV2012 keys when possible.
    if settings.ToneCurvePV2012 and settings.ExtendedToneCurvePV2012 == nil then
        settings.ExtendedToneCurvePV2012 = settings.ToneCurvePV2012
    end
    if settings.ToneCurvePV2012Red and settings.ExtendedToneCurvePV2012Red == nil then
        settings.ExtendedToneCurvePV2012Red = settings.ToneCurvePV2012Red
    end
    if settings.ToneCurvePV2012Green and settings.ExtendedToneCurvePV2012Green == nil then
        settings.ExtendedToneCurvePV2012Green = settings.ToneCurvePV2012Green
    end
    if settings.ToneCurvePV2012Blue and settings.ExtendedToneCurvePV2012Blue == nil then
        settings.ExtendedToneCurvePV2012Blue = settings.ToneCurvePV2012Blue
    end

    if settings.ToneCurvePV2012
        or settings.ToneCurvePV2012Red
        or settings.ToneCurvePV2012Green
        or settings.ToneCurvePV2012Blue
        or settings.ExtendedToneCurvePV2012
        or settings.ExtendedToneCurvePV2012Red
        or settings.ExtendedToneCurvePV2012Green
        or settings.ExtendedToneCurvePV2012Blue then
        settings.EnableToneCurve = true
        settings.ToneCurveName2012 = "Custom"
        settings.ToneCurveName = "Custom"
    end
    return settings
end

local function buildLensCorrectionSettings(lensCorrections, warnings)
    local settings = {}
    if type(lensCorrections) ~= "table" then
        return settings
    end
    if lensCorrections.enable_profile_corrections ~= nil then
        settings.EnableLensCorrections = lensCorrections.enable_profile_corrections
    end
    if lensCorrections.remove_chromatic_aberration ~= nil then
        settings.AutoLateralCA = lensCorrections.remove_chromatic_aberration
    end
    return settings
end

local function mergeSettings(target, source)
    for key, value in pairs(source or {}) do
        target[key] = value
    end
end

local function normalizeDevelopValue(key, value)
    if type(value) ~= "number" then
        return value
    end
    local bounds = DEVELOP_VALUE_BOUNDS[key]
    if not bounds then
        return value
    end
    if bounds.wrap then
        local span = bounds.max - bounds.min
        if span <= 0 then
            return value
        end
        local shifted = value - bounds.min
        local wrapped = shifted - math.floor(shifted / span) * span
        return bounds.min + wrapped
    end
    if value < bounds.min then
        return bounds.min
    end
    if value > bounds.max then
        return bounds.max
    end
    return value
end

local function mergeGlobalDevelopSettings(currentSettings, aiSettings)
    local merged = {}
    for key, value in pairs(aiSettings or {}) do
        if ADDITIVE_GLOBAL_KEYS[key] and type(value) == "number" then
            local baseValue = currentSettings and currentSettings[key]
            if type(baseValue) ~= "number" then
                baseValue = 0
            end
            merged[key] = normalizeDevelopValue(key, baseValue + value)
        else
            merged[key] = normalizeDevelopValue(key, value)
        end
    end
    return merged
end

local function formatGlobalSettings(globalSettings)
    local lines = {}
    for _, key in ipairs(sortedKeys(globalSettings or {})) do
        if key ~= "hsl" and key ~= "color_grading" and key ~= "tone_curve" and key ~= "lens_corrections" then
            table.insert(lines, "- " .. tostring(key) .. ": " .. tostring(globalSettings[key]))
        end
    end
    if type(globalSettings.hsl) == "table" then
        table.insert(lines, "- hsl: " .. tostring(tableCount(globalSettings.hsl)) .. " channel(s)")
    end
    if type(globalSettings.color_grading) == "table" then
        table.insert(lines, "- color_grading: enabled")
    end
    if type(globalSettings.tone_curve) == "table" then
        table.insert(lines, "- tone_curve: enabled")
    end
    if type(globalSettings.lens_corrections) == "table" then
        table.insert(lines, "- lens_corrections: enabled")
    end
    return lines
end

function DevelopEditManager.formatRecipeDetails(response)
    local recipe = getRecipeFromResponse(response)
    if not recipe then
        return "No edit recipe available."
    end

    local lines = {}
    table.insert(lines, "Summary")
    table.insert(lines, recipe.summary or "AI-generated Lightroom edit recipe")
    table.insert(lines, "")

    local globalSettings = recipe.global or {}
    table.insert(lines, "Global adjustments")
    local globalLines = formatGlobalSettings(globalSettings)
    if #globalLines == 0 then
        table.insert(lines, "- none")
    else
        for _, line in ipairs(globalLines) do
            table.insert(lines, line)
        end
    end
    table.insert(lines, "")

    table.insert(lines, "Masks")
    local masks = recipe.masks or {}
    if #masks == 0 then
        table.insert(lines, "- none")
    else
        for _, mask in ipairs(masks) do
            local count = tableCount(mask.adjustments or {})
            table.insert(lines, "- " .. tostring(mask.kind or "mask") .. " (" .. tostring(count) .. " adjustment(s))")
        end
    end
    table.insert(lines, "")

    table.insert(lines, "Warnings")
    local warnings = recipe.warnings or {}
    if #warnings == 0 then
        table.insert(lines, "- none")
    else
        for _, warning in ipairs(warnings) do
            table.insert(lines, "- " .. tostring(warning))
        end
    end

    return table.concat(lines, "\n")
end

function DevelopEditManager.persistEditRecipe(photo, response, warnings, status)
    log:trace("DevelopEditManager.persistEditRecipe: start status=" .. tostring(status))
    local okRecipe, recipeOrErr = LrTasks.pcall(function()
        return getRecipeFromResponse(response)
    end)
    if not okRecipe then
        log:error("DevelopEditManager.persistEditRecipe: getRecipeFromResponse failed: " .. tostring(recipeOrErr))
        return
    end
    local recipe = recipeOrErr
    if not photo or not recipe then
        log:error("DevelopEditManager.persistEditRecipe: missing photo or recipe")
        return
    end

    log:trace("DevelopEditManager.persistEditRecipe: recipe resolved, building warnings")
    local allWarnings = {}
    if type(recipe.warnings) == "table" then
        for _, warning in ipairs(recipe.warnings) do
            table.insert(allWarnings, tostring(warning))
        end
    end
    if type(warnings) == "table" then
        for _, warning in ipairs(warnings) do
            table.insert(allWarnings, tostring(warning))
        end
    end

    log:trace("DevelopEditManager.persistEditRecipe: encoding recipe JSON")
    local okEncode, recipeJsonOrErr = LrTasks.pcall(function()
        return JSON:encode(recipe)
    end)
    if not okEncode then
        log:error("DevelopEditManager.persistEditRecipe: JSON encode failed: " .. tostring(recipeJsonOrErr))
        recipeJsonOrErr = "{}"
    end
    local recipeJson = recipeJsonOrErr

    local warningText = #allWarnings > 0 and table.concat(allWarnings, "\n") or ""
    if #warningText > 500 then
        warningText = string.sub(warningText, 1, 500)
        appendWarning(allWarnings, "Warnings were truncated for Lightroom metadata field size limits.")
    end
    log:trace("DevelopEditManager.persistEditRecipe: warningText length=" .. tostring(#warningText))
    local runDate = (type(response) == "table" and (response.edit_rundate or response.ai_rundate)) or ""
    if runDate == "" then
        runDate = LrDate.timeToW3CDate(LrDate.currentTime())
    end
    local modelName = ""
    if type(response) == "table" then
        modelName = response.edit_model or response.ai_model or ""
    end

    log:trace("DevelopEditManager.persistEditRecipe: entering catalog write")
    local catalog = LrApplication.activeCatalog()
    local okWrite, writeErr = LrTasks.pcall(function()
        -- withPrivateWriteAccessDo signature here is (callback [, options]).
        -- Passing an action-name string first can trigger obscure runtime errors in LR.
        catalog:withPrivateWriteAccessDo(function()
            photo:setPropertyForPlugin(_PLUGIN, "aiEditLastRun", tostring(runDate))
            photo:setPropertyForPlugin(_PLUGIN, "aiEditModel", tostring(modelName))
            photo:setPropertyForPlugin(_PLUGIN, "aiEditSummary", tostring(recipe.summary or ""))
            photo:setPropertyForPlugin(_PLUGIN, "aiEditWarnings", warningText)
            photo:setPropertyForPlugin(_PLUGIN, "aiEditRecipe", tostring(recipeJson or ""))
            photo:setPropertyForPlugin(_PLUGIN, "aiEditStatus", tostring(status or "generated"))
        end, Defaults.catalogWriteAccessOptions)
    end)
    if not okWrite then
        log:error("DevelopEditManager.persistEditRecipe: catalog write failed: " .. tostring(writeErr))
        return
    end
    log:trace("DevelopEditManager.persistEditRecipe: done warningsCount=" .. tostring(#allWarnings))
end

local function buildDevelopSettings(recipe, warnings)
    local developSettings = {}
    local globalSettings = recipe.global or {}

    for key, lrKey in pairs(GLOBAL_KEY_MAP) do
        local value = globalSettings[key]
        if value ~= nil then
            developSettings[lrKey] = value
        end
    end

    mergeSettings(developSettings, buildHslDevelopSettings(globalSettings.hsl))
    mergeSettings(developSettings, buildColorGradingDevelopSettings(globalSettings.color_grading, warnings))
    mergeSettings(developSettings, buildToneCurveSettings(globalSettings.tone_curve))
    mergeSettings(developSettings, buildLensCorrectionSettings(globalSettings.lens_corrections, warnings))

    return developSettings
end

local function focusPhotoInDevelop(photo, warnings)
    local catalog = LrApplication.activeCatalog()
    local ok, err = LrTasks.pcall(function()
        catalog:setSelectedPhotos(photo, { photo })
        LrApplicationView.switchToModule("develop")
        LrTasks.sleep(0.2)
    end)
    if not ok then
        -- Fallback for cases where the current source doesn't contain the photo.
        local fallbackOk, fallbackErr = LrTasks.pcall(function()
            catalog:setActiveSources({ catalog.kAllPhotos })
            LrTasks.sleep(0.2)
            catalog:setSelectedPhotos(photo, { photo })
            LrApplicationView.switchToModule("develop")
            LrTasks.sleep(0.2)
        end)
        if fallbackOk then
            appendWarning(warnings, "Photo was not available in the current source; temporarily switched to All Photos for mask application.")
            return true
        end
        err = fallbackErr
    end
    if not ok then
        appendWarning(warnings, "Could not switch Lightroom to the Develop module for mask application: " .. tostring(err))
        return false
    end
    return true
end

local function applyGlobalDevelopSettings(photo, recipe, warnings)
    log:trace("DevelopEditManager.applyGlobalDevelopSettings: start")
    local developSettings = buildDevelopSettings(recipe, warnings)
    if next(developSettings) == nil then
        log:trace("DevelopEditManager.applyGlobalDevelopSettings: nothing to apply")
        return true
    end

    local mergedSettings = developSettings
    local okCurrent, currentOrErr = LrTasks.pcall(function()
        return photo:getDevelopSettings()
    end)
    if okCurrent and type(currentOrErr) == "table" then
        mergedSettings = mergeGlobalDevelopSettings(currentOrErr, developSettings)
    else
        appendWarning(warnings, "Could not read current develop settings for additive merge; AI edits were applied directly.")
        log:warn("DevelopEditManager.applyGlobalDevelopSettings current settings unavailable: " .. tostring(currentOrErr))
    end

    local catalog = LrApplication.activeCatalog()
    local ok, err = LrTasks.pcall(function()
        catalog:withWriteAccessDo("Apply AI Lightroom develop settings", function()
            photo:applyDevelopSettings(mergedSettings)
        end, Defaults.catalogWriteAccessOptions)
    end)
    if not ok then
        appendWarning(warnings, "Failed to apply global develop settings: " .. tostring(err))
        log:error("DevelopEditManager.applyGlobalDevelopSettings failed: " .. tostring(err))
        return false
    end
    log:trace("DevelopEditManager.applyGlobalDevelopSettings: success")
    return true
end

local function supportsMaskAutomation()
    return type(LrDevelopController) == "table"
        and type(LrDevelopController.createNewMask) == "function"
end

local function applyMaskEdits(photo, recipe, warnings)
    log:trace("DevelopEditManager.applyMaskEdits: start")
    local masks = recipe.masks or {}
    if #masks == 0 then
        log:trace("DevelopEditManager.applyMaskEdits: no masks")
        return true
    end

    if not supportsMaskAutomation() then
        appendWarning(warnings, "Lightroom mask automation is unavailable in this Lightroom SDK version. Mask edits were stored but not applied.")
        log:warn("DevelopEditManager.applyMaskEdits: mask automation unavailable")
        return false
    end

    if not focusPhotoInDevelop(photo, warnings) then
        return false
    end
    if type(LrDevelopController.goToMasking) == "function" then
        LrTasks.pcall(function()
            LrDevelopController.goToMasking()
        end)
    end

    local function findMaskGroup(settings)
        if type(settings) ~= "table" then
            return nil, nil
        end
        if type(settings.MaskGroup) == "table" then
            return "MaskGroup", settings.MaskGroup
        end
        if type(settings.MaskGroupBasedCorrections) == "table" then
            return "MaskGroupBasedCorrections", settings.MaskGroupBasedCorrections
        end
        return nil, nil
    end

    local function logMaskedDevelopSnapshot(stageLabel)
        local ok, settingsOrErr = LrTasks.pcall(function()
            return photo:getDevelopSettings()
        end)
        if not ok or type(settingsOrErr) ~= "table" then
            log:trace("DevelopEditManager.applyMaskEdits snapshot(" .. tostring(stageLabel) .. "): no develop settings")
            return
        end
        local groupKey, masks = findMaskGroup(settingsOrErr)
        if type(masks) ~= "table" then
            local maskLikeKeys = {}
            for key, value in pairs(settingsOrErr) do
                if type(key) == "string" and string.find(key, "Mask") and type(value) == "table" then
                    table.insert(maskLikeKeys, key)
                end
            end
            table.sort(maskLikeKeys)
            log:trace("DevelopEditManager.applyMaskEdits snapshot(" .. tostring(stageLabel) .. "): no mask group found; mask-like keys=" .. tostring(table.concat(maskLikeKeys, ",")))
            return
        end
        local dump = ""
        local okDump, dumpOrErr = LrTasks.pcall(function()
            return Util.dumpTable(masks)
        end)
        if okDump and type(dumpOrErr) == "string" then
            dump = dumpOrErr
        end
        if #dump > 1800 then
            dump = string.sub(dump, 1, 1800) .. "...(truncated)"
        end
        log:trace("DevelopEditManager.applyMaskEdits snapshot(" .. tostring(stageLabel) .. "): group=" .. tostring(groupKey) .. " maskCount=" .. tostring(#masks) .. " masks=" .. tostring(dump))
    end

    local function applyMaskAdjustmentsViaDevelopSettings(maskKind, adjustments)
        if type(adjustments) ~= "table" then
            return false, "no adjustments"
        end
        local catalog = LrApplication.activeCatalog()
        local ok, err = LrTasks.pcall(function()
            catalog:withWriteAccessDo("Apply AI mask adjustments via develop settings", function()
                local settings = photo:getDevelopSettings()
                local _, maskGroup = findMaskGroup(settings)
                if type(maskGroup) ~= "table" or #maskGroup == 0 then
                    error("mask group not available in develop settings")
                end

                -- Newly created mask is typically appended; target the latest one.
                local targetMask = maskGroup[#maskGroup]
                if type(targetMask) ~= "table" then
                    error("last mask entry is not a table")
                end
                local correction = targetMask.Correction
                if type(correction) ~= "table" then
                    correction = targetMask.correction
                end
                if type(correction) ~= "table" then
                    correction = targetMask.Adjustments
                end
                if type(correction) ~= "table" then
                    correction = targetMask.adjustments
                end
                if type(correction) ~= "table" then
                    correction = {}
                    targetMask.Correction = correction
                end

                for key, value in pairs(adjustments) do
                    local candidates = MASK_KEY_CANDIDATES[key]
                    local written = false
                    if candidates and #candidates > 0 then
                        for _, candidate in ipairs(candidates) do
                            correction[candidate] = value
                            written = true
                        end
                    end
                    if not written then
                        appendWarning(warnings, "Mask adjustment '" .. tostring(key) .. "' is not currently supported and was ignored.")
                    end
                end

                photo:applyDevelopSettings(settings)
                if type(photo.updateAISettings) == "function" then
                    photo:updateAISettings()
                end
            end, Defaults.catalogWriteAccessOptions)
        end)
        if not ok then
            return false, err
        end
        log:trace("DevelopEditManager.applyMaskEdits applied adjustments via develop settings for mask kind=" .. tostring(maskKind))
        return true, nil
    end

    local function readMaskList()
        if type(LrDevelopController.getAllMasks) ~= "function" then
            return {}
        end
        local ok, masksOrErr = LrTasks.pcall(function()
            return LrDevelopController.getAllMasks()
        end)
        if not ok or type(masksOrErr) ~= "table" then
            return {}
        end
        return masksOrErr
    end

    local function extractMaskId(maskItem)
        if type(maskItem) == "string" or type(maskItem) == "number" then
            return tostring(maskItem)
        end
        if type(maskItem) == "table" then
            if maskItem.id ~= nil then return tostring(maskItem.id) end
            if maskItem.maskId ~= nil then return tostring(maskItem.maskId) end
            if maskItem.uuid ~= nil then return tostring(maskItem.uuid) end
        end
        return nil
    end

    local function buildMaskIdSet(maskList)
        local ids = {}
        for _, item in ipairs(maskList or {}) do
            local id = extractMaskId(item)
            if id then
                ids[id] = true
            end
        end
        return ids
    end

    local function findNewMaskId(beforeMasks, afterMasks)
        local beforeIds = buildMaskIdSet(beforeMasks)
        for _, item in ipairs(afterMasks or {}) do
            local id = extractMaskId(item)
            if id and not beforeIds[id] then
                return id
            end
        end
        return nil
    end

    local function selectMaskById(maskId)
        if not maskId or type(LrDevelopController.selectMask) ~= "function" then
            return false
        end
        local ok = LrTasks.pcall(function()
            LrDevelopController.selectMask(maskId)
        end)
        return ok == true
    end

    local function getSelectedMaskId()
        if type(LrDevelopController.getSelectedMask) ~= "function" then
            return nil
        end
        local ok, selectedOrErr = LrTasks.pcall(function()
            return LrDevelopController.getSelectedMask()
        end)
        if not ok then
            return nil
        end
        local selectedDump = ""
        local okDump, dumpOrErr = LrTasks.pcall(function()
            return Util.dumpTable(selectedOrErr)
        end)
        if okDump and type(dumpOrErr) == "string" then
            selectedDump = dumpOrErr
            if #selectedDump > 800 then
                selectedDump = string.sub(selectedDump, 1, 800) .. "...(truncated)"
            end
        end
        log:trace("DevelopEditManager.applyMaskEdits selectedMask raw=" .. tostring(selectedDump))
        return extractMaskId(selectedOrErr)
    end

    local function getAiMaskToolCandidates(maskKind)
        local key = string.lower(tostring(maskKind or ""))
        local mapped = AI_MASK_TOOL_CANDIDATES[key]
        if mapped and #mapped > 0 then
            return mapped
        end
        return { key }
    end

    local function selectAiMaskTool(toolToken)
        if type(LrDevelopController.selectMaskTool) ~= "function" then
            return false, "selectMaskTool unavailable"
        end
        local okOneArg, errOneArg = LrTasks.pcall(function()
            LrDevelopController.selectMaskTool(toolToken)
        end)
        if okOneArg then
            return true, nil
        end
        local okTwoArgs, errTwoArgs = LrTasks.pcall(function()
            LrDevelopController.selectMaskTool("aiSelection", toolToken)
        end)
        if okTwoArgs then
            return true, nil
        end
        return false, errTwoArgs or errOneArg
    end

    local function createAiSelectionMask(toolToken)
        local okWithHint, idOrErrWithHint = LrTasks.pcall(function()
            return LrDevelopController.createNewMask("aiSelection", toolToken)
        end)
        if okWithHint then
            return true, extractMaskId(idOrErrWithHint), nil
        end
        local okNoHint, idOrErrNoHint = LrTasks.pcall(function()
            return LrDevelopController.createNewMask("aiSelection")
        end)
        if okNoHint then
            return true, extractMaskId(idOrErrNoHint), nil
        end
        return false, nil, idOrErrWithHint or idOrErrNoHint
    end

    local function createMaskForKind(maskKind)
        local toolCandidates = getAiMaskToolCandidates(maskKind)
        local lastAiErr = nil
        for _, toolToken in ipairs(toolCandidates) do
            local selectedTool, selectErr = selectAiMaskTool(toolToken)
            if not selectedTool then
                lastAiErr = selectErr
            end
            local created, createdMaskId, createErr = createAiSelectionMask(toolToken)
            if created then
                log:trace("DevelopEditManager.applyMaskEdits create mask kind=" .. tostring(maskKind) .. " using ai tool token=" .. tostring(toolToken) .. " selectedTool=" .. tostring(selectedTool) .. " createdMaskId=" .. tostring(createdMaskId))
                return true, createdMaskId, nil
            end
            lastAiErr = createErr or lastAiErr
            log:trace("DevelopEditManager.applyMaskEdits ai create failed kind=" .. tostring(maskKind) .. " token=" .. tostring(toolToken) .. " err=" .. tostring(createErr))
        end

        local okBrush, errBrush = LrTasks.pcall(function()
            return LrDevelopController.createNewMask("brush")
        end)
        if okBrush then
            appendWarning(warnings, "Mask kind '" .. tostring(maskKind) .. "' fell back to brush; refine manually.")
            return true, extractMaskId(errBrush), nil
        end

        return false, nil, lastAiErr or errBrush
    end

    local function waitForMaskId(beforeMasks, immediateMaskId)
        if immediateMaskId then
            return immediateMaskId
        end
        for _ = 1, 12 do
            local masksAfter = readMaskList()
            local newMaskId = findNewMaskId(beforeMasks, masksAfter)
            if newMaskId then
                return newMaskId
            end
            local selectedMaskId = getSelectedMaskId()
            if selectedMaskId then
                return selectedMaskId
            end
            if #masksAfter > 0 then
                local lastId = extractMaskId(masksAfter[#masksAfter])
                if lastId then
                    return lastId
                end
            end
            LrTasks.sleep(0.1)
        end
        return nil
    end

    for _, mask in ipairs(masks) do
        local maskKind = tostring(mask.kind or "")
        local ok, err = LrTasks.pcall(function()
            logMaskedDevelopSnapshot("before_" .. maskKind)
            local masksBefore = readMaskList()
            local created, createdMaskId, createErr = createMaskForKind(maskKind)
            if not created then
                error("createNewMask failed: " .. tostring(createErr))
            end
            local newMaskId = waitForMaskId(masksBefore, createdMaskId)
            local hasMaskContext = false
            if newMaskId then
                local selected = selectMaskById(newMaskId)
                hasMaskContext = selected or type(LrDevelopController.selectMask) ~= "function"
                log:trace("DevelopEditManager.applyMaskEdits created mask kind=" .. tostring(maskKind) .. " newMaskId=" .. tostring(newMaskId) .. " selectOk=" .. tostring(selected))
            else
                hasMaskContext = false
                log:trace("DevelopEditManager.applyMaskEdits created mask kind=" .. tostring(maskKind) .. " but could not identify new mask id")
            end
            logMaskedDevelopSnapshot("after_create_" .. maskKind)

            -- AI mask generation can complete asynchronously; give LR a moment.
            LrTasks.sleep(0.35)

            -- Best-effort to ensure local adjustment context is active.
            LrTasks.pcall(function()
                LrDevelopController.setValue("local_Amount", 100)
            end)
            local shouldInvert = mask.invert or (string.lower(maskKind) == "background")
            if shouldInvert and type(LrDevelopController.toggleInvertMaskTool) == "function" then
                LrDevelopController.toggleInvertMaskTool()
            end
            local controllerAppliedCount = 0
            if type(LrDevelopController.setValue) == "function" then
                for key, value in pairs(mask.adjustments or {}) do
                    local candidates = MASK_KEY_CANDIDATES[key]
                    if candidates and #candidates > 0 then
                        local applied = false
                        local lastErr = nil
                        for _, candidate in ipairs(candidates) do
                            local setOk, setErr = LrTasks.pcall(function()
                                LrDevelopController.setValue(candidate, value)
                            end)
                            if setOk then
                                local readBack = nil
                                local readBackOk = false
                                if type(LrDevelopController.getValue) == "function" then
                                    local rbOk, rbVal = LrTasks.pcall(function()
                                        return LrDevelopController.getValue(candidate)
                                    end)
                                    if rbOk then
                                        readBack = rbVal
                                        readBackOk = true
                                    end
                                end
                                -- Lightroom may apply local mask adjustments even when getValue() cannot
                                -- read the local slider (returns nil on some SDK versions).
                                applied = hasMaskContext
                                if hasMaskContext then
                                    controllerAppliedCount = controllerAppliedCount + 1
                                end
                                if readBackOk then
                                    log:trace("DevelopEditManager.applyMaskEdits applied " .. tostring(key) .. " via " .. tostring(candidate) .. "=" .. tostring(value) .. " readBack=" .. tostring(readBack))
                                else
                                    log:trace("DevelopEditManager.applyMaskEdits applied " .. tostring(key) .. " via " .. tostring(candidate) .. "=" .. tostring(value) .. " readBack=unavailable")
                                end
                                if not hasMaskContext then
                                    log:trace("DevelopEditManager.applyMaskEdits mask context missing while setting " .. tostring(candidate) .. "; treating as unverified")
                                end
                                break
                            else
                                lastErr = setErr
                                log:trace("DevelopEditManager.applyMaskEdits candidate failed " .. tostring(key) .. " via " .. tostring(candidate) .. ": " .. tostring(setErr))
                            end
                        end
                        if not applied then
                            appendWarning(warnings, "Mask adjustment '" .. tostring(key) .. "' could not be applied for " .. maskKind .. ": " .. tostring(lastErr or "unknown error"))
                        end
                    else
                        appendWarning(warnings, "Mask adjustment '" .. tostring(key) .. "' is not currently supported and was ignored.")
                    end
                end
            else
                log:trace("DevelopEditManager.applyMaskEdits: LrDevelopController.setValue unavailable; relying on develop-settings fallback")
            end

            -- Avoid clobbering controller-applied local slider values with a stale
            -- develop-settings snapshot. Only run the fallback when controller writes
            -- were not successfully applied.
            if controllerAppliedCount == 0 or not hasMaskContext then
                local fallbackOk, fallbackErr = applyMaskAdjustmentsViaDevelopSettings(maskKind, mask.adjustments or {})
                if not fallbackOk then
                    appendWarning(warnings, "Mask adjustments for '" .. maskKind .. "' could not be persisted via develop settings: " .. tostring(fallbackErr))
                end
            end
            logMaskedDevelopSnapshot("after_adjust_" .. maskKind)
        end)
        if not ok then
            appendWarning(warnings, "Mask '" .. maskKind .. "' could not be applied: " .. tostring(err))
            log:error("DevelopEditManager.applyMaskEdits mask failed: " .. tostring(maskKind) .. " err=" .. tostring(err))
        end
    end

    -- Leave masking UI so users return to normal Develop controls.
    if type(LrDevelopController.selectTool) == "function" then
        local okExit, exitErr = LrTasks.pcall(function()
            LrDevelopController.selectTool("loupe")
        end)
        if not okExit then
            log:trace("DevelopEditManager.applyMaskEdits: could not exit masking mode: " .. tostring(exitErr))
        end
    end

    log:trace("DevelopEditManager.applyMaskEdits: done")
    return true
end

function DevelopEditManager.showValidationDialog(context, photo, response, options)
    log:trace("DevelopEditManager.showValidationDialog: start")
    local recipe = getRecipeFromResponse(response)
    if not recipe then
        log:error("DevelopEditManager.showValidationDialog: no recipe in response")
        return "cancel", nil
    end

    local f = LrView.osFactory()
    local bind = LrView.bind
    local props = LrBinding.makePropertyTable(context)
    props.applyGlobal = next(recipe.global or {}) ~= nil
    props.applyMasks = (options and options.applyMasks ~= false) and ((recipe.masks and #recipe.masks > 0) or false)
    props.details = DevelopEditManager.formatRecipeDetails(response)

    local dialogView = f:column {
        bind_to_object = props,
        spacing = f:control_spacing(),
        f:row {
            f:static_text {
                title = photo:getFormattedMetadata("fileName") or "Photo",
            },
        },
        f:row {
            f:checkbox { value = bind "applyGlobal" },
            f:static_text { title = "Apply global develop settings" },
        },
        f:row {
            f:checkbox {
                value = bind "applyMasks",
                enabled = (recipe.masks and #recipe.masks > 0) or false,
            },
            f:static_text { title = "Apply masks when possible" },
        },
        f:row {
            f:edit_field {
                value = bind "details",
                width_in_chars = 70,
                height_in_lines = 22,
            },
        },
    }

    local result = LrDialogs.presentModalDialog({
        title = "Review AI Lightroom Edit",
        contents = dialogView,
        actionVerb = "Apply",
    })
    log:trace("DevelopEditManager.showValidationDialog: result=" .. tostring(result))

    if result == "ok" then
        return result, {
            applyGlobal = props.applyGlobal,
            applyMasks = props.applyMasks,
        }
    end
    return result, nil
end

function DevelopEditManager.applyRecipe(photo, response, options)
    log:trace("DevelopEditManager.applyRecipe: start")
    local recipe = getRecipeFromResponse(response)
    if not recipe then
        log:error("DevelopEditManager.applyRecipe: no recipe")
        return false, { "No edit recipe returned by the AI." }
    end

    local warnings = {}
    if type(recipe.warnings) == "table" then
        for _, warning in ipairs(recipe.warnings) do
            table.insert(warnings, tostring(warning))
        end
    end

    local applyGlobal = options == nil or options.applyGlobal ~= false
    local applyMasks = options ~= nil and options.applyMasks == true

    local globalApplied = true
    if applyGlobal then
        globalApplied = applyGlobalDevelopSettings(photo, recipe, warnings)
    end
    if applyMasks then
        applyMaskEdits(photo, recipe, warnings)
    end

    DevelopEditManager.persistEditRecipe(photo, response, warnings, "applied")
    log:trace("DevelopEditManager.applyRecipe: done globalApplied=" .. tostring(globalApplied) .. " warningsCount=" .. tostring(#warnings))
    return globalApplied, warnings
end
