require "Info"

UpdateCheck = {}

UpdateCheck.releaseTagName = "v" .. tostring(Info.MAJOR) .. "." .. tostring(Info.MINOR) .. "." .. tostring(Info.REVISION)
UpdateCheck.updateCheckUrl = "https://api.github.com/repos/LrGenius/LrGeniusAI/releases/latest"
UpdateCheck.latestReleaseUrl = "https://github.com/LrGenius/LrGeniusAI/releases/latest"

function UpdateCheck.checkForNewVersion()
    local response, headers = LrHttp.get(UpdateCheck.updateCheckUrl)

    if headers.status == 200 then
        if response ~= nil then
            local decoded = JSON:decode(response)
            if decoded ~= nil then
                if decoded.tag_name ~= UpdateCheck.releaseTagName then
                    LrHttp.openUrlInBrowser(UpdateCheck.latestReleaseUrl)
                else
                    LrDialogs.message("You're on the latest plugin version: " .. UpdateCheck.releaseTagName)
                end
            end
        else
            log:error('Could not run update check. Empty response')
        end
    else
        log:error('Update check failed. ' .. UpdateCheck.updateCheckUrl)
        log:error(Util.dumpTable(headers))
        log:error(response)
        return nil
    end
    return nil
end

function UpdateCheck.checkForNewVersionInBackground()
    local response, headers = LrHttp.get(UpdateCheck.updateCheckUrl)

    if headers.status == 200 then
        if response ~= nil then
            local decoded = JSON:decode(response)
            if decoded ~= nil then
                if decoded.tag_name ~= UpdateCheck.releaseTagName then
                    LrDialogs.message("A new version of LrGeniusAI is available: " .. decoded.tag_name .. ". Please visit the releases page to download the latest version.", "New Version Available", "info")
                else
                    LrDialogs.message("You're on the latest plugin version: " .. UpdateCheck.releaseTagName)
                end
            end
        else
            log:error('Could not run update check. Empty response')
        end
    else
        log:error('Update check failed. ' .. UpdateCheck.updateCheckUrl)
        log:error(Util.dumpTable(headers))
        log:error(response)
        return nil
    end
    return nil
end