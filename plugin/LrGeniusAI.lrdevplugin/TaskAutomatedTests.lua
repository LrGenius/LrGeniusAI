-- TaskAutomatedTests.lua
-- Runs the plugin's in-Lightroom tests (PluginTests.lua) and shows the result.
-- The same tests run headless through the agent bridge (run_plugin_tests).

local LrTasks = import("LrTasks")
local LrDialogs = import("LrDialogs")
local LrFunctionContext = import("LrFunctionContext")

LrTasks.startAsyncTask(function()
	LrFunctionContext.callWithContext("automatedTestsTask", function(context)
		LrDialogs.attachErrorDialogToFunctionContext(context)
		local choice = LrDialogs.confirm(
			"Run the plugin's tests?",
			"They check Lightroom's Lua environment, the backend connection, the catalog and the agent "
				.. "bridge commands.\n\nThe catalog-writing tests change the selected photo (or the "
				.. "first photo) and put every change back.",
			"Run tests",
			"Cancel",
			"Run including catalog writes"
		)
		if choice == "cancel" then
			return
		end

		local scope = LrProgressScope({ title = "LrGeniusAI tests", functionContext = context })
		local summary = PluginTests.run({
			includeWrites = choice == "other",
			allowChanges = true,
			progress = function(done, total, message)
				scope:setPortionComplete(done, total)
				scope:setCaption(message)
				return scope:isCanceled()
			end,
		})
		scope:done()

		local lines = {
			string.format(
				"Passed: %d   Failed: %d   Skipped: %d   (%d ms)",
				summary.passed,
				summary.failed,
				summary.skipped,
				summary.duration_ms
			),
		}
		local failures = {}
		for _, result in ipairs(summary.results) do
			if result.ok == false then
				failures[#failures + 1] = result.suite .. " / " .. result.name .. ":\n   " .. result.error
			end
		end
		for i = 1, math.min(#failures, 6) do
			lines[#lines + 1] = ""
			lines[#lines + 1] = failures[i]
		end
		if #failures > 6 then
			lines[#lines + 1] = string.format("\n... and %d more failures (see the log).", #failures - 6)
		end
		for _, note in ipairs(summary.leftovers or {}) do
			lines[#lines + 1] = "\nNote: " .. note
		end

		if summary.failed > 0 then
			ErrorHandler.handleError("Some tests failed", table.concat(lines, "\n"))
		else
			LrDialogs.message("All tests passed", table.concat(lines, "\n"), "info")
		end
	end)
end)
