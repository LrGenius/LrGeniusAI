require "Info"

UpdateCheck = {}

UpdateCheck.releaseTagName = tostring(Info.MAJOR) .. "." .. tostring(Info.MINOR) .. "." .. tostring(Info.REVISION)
UpdateCheck.updateCheckUrl = ""
UpdateCheck.latestReleaseUrl = "https://github.com/LrGenius"

function UpdateCheck.checkForNewVersion()
    -- local response, headers = LrHttp.get(UpdateCheck.updateCheckUrl)

    -- if headers.status == 200 then
    --     if response ~= nil then
    --         local remoteVersionString = Util.trim(response)
    --         if remoteVersionString ~= UpdateCheck.releaseTagName then
    --             if LrDialogs.confirm(LOC "$$$/lrc-ai-assistant/UpdateCheck/newVersionAvailable=A new version of LrGeniusAI is available.", 
    --                 "Version " .. remoteVersionString .. LOC "$$$/lrc-ai-assistant/UpdateCheck/isAvailable= is available.",
    --                 LOC "$$$/lrc-ai-assistant/UpdateCheck/closeAndUpdate=Close Lightroom and Update now",
    --                 LOC "$$$/lrc-ai-assistant/UpdateCheck/notNow=Not now"
    --             ) == "ok" then
    --                 LrTasks.startAsyncTask(function()
    --                     UPDATE_COMMAND = nil
    --                     if WIN_ENV then
    --                         UPDATE_COMMAND = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "LrGeniusAI-Update-Tool.exe")
    --                     else
    --                         UPDATE_COMMAND = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "LrGeniusAI-Update-Tool.app")
    --                     end
    --                     if UPDATE_COMMAND ~= nil and LrFileUtils.exists(UPDATE_COMMAND) then
    --                         if MAC_ENV then
    --                             UPDATE_COMMAND = "open " .. UPDATE_COMMAND
    --                         end
    --                         log:trace("Trying to launch update tool: " .. UPDATE_COMMAND)
    --                         LrTasks.execute(UPDATE_COMMAND)
    --                         LrApplication.shutdown()
    --                     else
    --                         ErrorHandler.handleError("Could not find update tool", tostring(UPDATE_COMMAND))
    --                     end
    --                 end)
    --             end
    --         else
    --             LrDialogs.message(LOC "$$$/lrc-ai-assistant/UpdateCheck/onCurrentVersion=You're running the current version of LrGeniusAI" .. " " .. UpdateCheck.releaseTagName)
    --         end
    --     else
    --         log:error('Could not run update check. Empty response')
    --     end
    -- else
    --     log:error('Update check failed. ' .. UpdateCheck.updateCheckUrl)
    --     log:error(Util.dumpTable(headers))
    --     log:error(response)
    --     return nil
    -- end
    return nil
end

function UpdateCheck.checkForNewVersionInBackground()
    -- local response, headers = LrHttp.get(UpdateCheck.updateCheckUrl)

    -- if headers.status == 200 then
    --     if response ~= nil then
    --         local remoteVersionString = Util.trim(response)
    --         if remoteVersionString ~= UpdateCheck.releaseTagName then
    --             local result = LrDialogs.confirm(LOC "$$$/lrc-ai-assistant/UpdateCheck/newVersionAvailable=A new version of LrGeniusAI is available.",
    --                 "Version " .. remoteVersionString .. LOC "$$$/lrc-ai-assistant/UpdateCheck/isAvailable= is available.",
    --                 LOC "$$$/lrc-ai-assistant/UpdateCheck/closeAndUpdate=Close Lightroom and Update now",
    --                 LOC "$$$/lrc-ai-assistant/UpdateCheck/notNow=Not now",
    --                 LOC "$$$/lrc-ai-assistant/UpdateCheck/disableUpdateCheck=Disable update check"
    --             )
    --             if result == "ok" then
    --                 LrTasks.startAsyncTask(function()
    --                     UPDATE_COMMAND = nil
    --                     if WIN_ENV then
    --                         UPDATE_COMMAND = LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "LrGeniusAI-Update-Tool.exe")
    --                     else
    --                         UPDATE_COMMAND = "open " .. LrPathUtils.child(LrPathUtils.parent(_PLUGIN.path), "LrGeniusAI-Update-Tool.app")
    --                     end
    --                     if UPDATE_COMMAND ~= nil and LrFileUtils.exists(UPDATE_COMMAND) then
    --                         log:trace("Trying to launch update tool: " .. UPDATE_COMMAND)
    --                         LrTasks.execute(UPDATE_COMMAND)
    --                         LrApplication.shutdown()
    --                     else
    --                         ErrorHandler.handleError("Could not find update tool", tostring(UPDATE_COMMAND))
    --                     end
    --                 end)
    --             elseif result == "other" then
    --                 prefs.periodicalUpdateCheck = false
    --             end
    --         end
    --     else
    --         log:error('Could not run update check. Empty response')
    --     end
    -- else
    --     log:error('Update check failed. ' .. UpdateCheck.updateCheckUrl)
    --     log:error(Util.dumpTable(headers))
    --     log:error(response)
    --     return nil
    -- end
    return nil
end