---@diagnostic disable: undefined-global
---luacheck: globals TaskUpdate UpdateCheck SearchIndexAPI log LrFunctionContext LrProgressScope LrDialogs LrTasks LrPathUtils LrApplication

--[[
TaskUpdate.lua

Handles the code-only in-place update flow for LrGeniusAI.
Delegates the actual file operations to the Backend server and the external Updater GUI.

Flow:
  1. Guard: local backend only
  2. Fetch and validate the update manifest
  3. Show a confirmation dialog
  4. Send update request to backend (triggers the external Updater GUI)
  5. Inform user to close Lightroom — no public SDK API exists to quit Lightroom.
--]]

require("UpdateCheck")

TaskUpdate = {}

--- Formats a byte size for human display (KB / MB).
local function formatSize(bytes)
	if bytes >= 1024 * 1024 then
		return string.format("%.1f MB", bytes / (1024 * 1024))
	end
	return string.format("%.0f KB", bytes / 1024)
end

--- Main entry point. Accepts the release info table from UpdateCheck.getLatestReleaseInfo().
function TaskUpdate.runUpdate(releaseInfo)
	if not releaseInfo or not releaseInfo.manifest_url then
		LrDialogs.message(
			LOC("$$$/LrGeniusAI/TaskUpdate/ErrorTitle=Update Error"),
			LOC(
				"$$$/LrGeniusAI/TaskUpdate/NoManifestError=No code-only update manifest found. Please download the full installer from the releases page."
			),
			"critical"
		)
		return
	end

	LrTasks.startAsyncTask(function()
		-- Step 1: Guard — automated update only works against a local backend.
		-- Check this first so remote-backend users see the right message immediately
		-- without incurring a network download first.
		if not SearchIndexAPI.isLocalBackend() then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/TaskUpdate/ErrorTitle=Update Error"),
				LOC(
					"$$$/LrGeniusAI/TaskUpdate/RemoteBackendError=The automated update is only available for local backends. Please update the backend manually on your remote server and re-install the plugin if necessary."
				),
				"critical"
			)
			return
		end

		-- Step 2: Fetch manifest
		local manifest = UpdateCheck.fetchManifest(releaseInfo.manifest_url)
		if not manifest then
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/TaskUpdate/ErrorTitle=Update Error"),
				LOC(
					"$$$/LrGeniusAI/TaskUpdate/ManifestFetchError=Could not download the update manifest. Please check your internet connection."
				),
				"critical"
			)
			return
		end

		local version = manifest.version or releaseInfo.tag_name or "?"
		local totalSize = manifest.total_size_bytes or 0
		local pluginCount = (manifest.file_counts or {}).plugin or 0
		local backendCount = (manifest.file_counts or {}).backend_src or 0

		-- Step 3: Confirmation dialog
		local detail = LOC(
			"$$$/LrGeniusAI/TaskUpdate/ConfirmMsgBackend=The backend will download and replace the code files. You should close Lightroom once the process finishes."
		) .. "\n\n" .. LOC("$$$/LrGeniusAI/TaskUpdate/PluginFiles=Plugin files:") .. " " .. tostring(pluginCount) .. "   " .. LOC(
			"$$$/LrGeniusAI/TaskUpdate/BackendFiles=Backend files:"
		) .. " " .. tostring(backendCount) .. "   (" .. formatSize(totalSize) .. ")"

		local btn = LrDialogs.confirm(
			LOC("$$$/LrGeniusAI/TaskUpdate/ConfirmTitle=Install Update ^1?", version),
			detail,
			LOC("$$$/LrGeniusAI/TaskUpdate/Install=Install"),
			LOC("$$$/LrGeniusAI/common/Cancel=Cancel")
		)

		if btn ~= "ok" then
			return
		end

		-- Step 4: Apply update via backend (triggers the external GUI)
		local ok, result = SearchIndexAPI.applyUpdate(manifest)

		if not ok then
			log:error("TaskUpdate: backend update failed to start: " .. tostring(result))
			LrDialogs.message(
				LOC("$$$/LrGeniusAI/TaskUpdate/ErrorTitle=Update Error"),
				LOC(
					"$$$/LrGeniusAI/TaskUpdate/UpdateFailed=The update could not be started:\n\n^1",
					tostring(result or LOC("$$$/LrGeniusAI/common/UnknownError=Unknown error"))
				),
				"critical"
			)
			return
		end

		-- Step 5: Inform user — LrApplication has no public API to quit Lightroom.
		log:info("TaskUpdate: update to " .. version .. " triggered successfully")
		LrDialogs.message(
			LOC("$$$/LrGeniusAI/TaskUpdate/SuccessTitle=Update Successful"),
			LOC(
				"$$$/LrGeniusAI/TaskUpdate/ExternalUpdaterMsg=The update process has started in a separate window. Please close Lightroom now to allow the update to complete.\n\nYou can restart Lightroom once the update window shows 'Finished'."
			)
		)
	end)
end
