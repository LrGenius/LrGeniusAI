---
-- Shared view-model for the two local LLM backends (llama.cpp and MLX).
--
-- Both the plug-in settings dialog and the onboarding wizard need the same
-- picture of "what can this machine run, what is installed, what can I
-- download", so the mapping from /v1/llm/catalog + /v1/llm/status onto property-table
-- fields lives here instead of being written twice.
--
-- Fields written into the caller's property table:
--   llmSupported, llmStatusText, llmInstalledText, llmDownloadChoices, llmDownloadChoice
--   mlxSupported, mlxStatusText, mlxInstalledText, mlxDownloadChoices, mlxDownloadChoice
--
LocalModelCatalog = {}

---
-- Seeds the fields with safe "still checking" values.
--
-- The `*Supported` gates start false so nothing is clickable until the backend
-- has confirmed it can actually run that engine; the status line stays readable
-- and says why the rest is greyed out. Hiding the controls outright is not an
-- option: per the SDK, a hidden view still occupies its full layout space,
-- which would leave a blank hole.
--
function LocalModelCatalog.initFields(propertyTable)
	propertyTable.llmDownloadChoices = {}
	propertyTable.llmDownloadChoice = nil
	propertyTable.llmStatusText = LOC("$$$/LrGeniusAI/LocalModel/Checking=Checking for local models...")
	propertyTable.llmInstalledText = ""
	propertyTable.llmSupported = false

	-- MLX is deliberately a separate group from the llama.cpp one: the two hold
	-- different model formats (a GGUF file versus a model directory) and can
	-- each have their own model resident, so merging them into one control
	-- would misrepresent what is installed.
	propertyTable.mlxDownloadChoices = {}
	propertyTable.mlxDownloadChoice = nil
	propertyTable.mlxStatusText = LOC("$$$/LrGeniusAI/MlxModel/Checking=Checking for MLX models...")
	propertyTable.mlxInstalledText = ""
	propertyTable.mlxSupported = false
end

-- Formats a catalog entry as a picker item, appending the download size.
local function toDownloadChoices(entries)
	local choices = {}
	for _, entry in ipairs(entries or {}) do
		if not entry.installed then
			local gb = entry.approx_bytes and string.format(" (%.1f GB)", entry.approx_bytes / 1e9) or ""
			table.insert(choices, { title = (entry.label or entry.id) .. gb, value = entry.id })
		end
	end
	return choices
end

-- Keeps the picker's selection pointing at something the list still offers.
--
-- Callers refresh periodically, and a model that finishes downloading drops out
-- of `downloadable`; the id selected before would otherwise linger and keep the
-- Download button live for an entry that is no longer there.
local function pickValidChoice(choices, current)
	for _, choice in ipairs(choices) do
		if choice.value == current then
			return current
		end
	end
	return choices[1] and choices[1].value or nil
end

-- The llama.cpp half of /v1/llm/catalog + /v1/llm/status.
local function updateLlamaCppFields(propertyTable, catalog, status)
	if catalog.supported == false then
		propertyTable.llmStatusText = LOC(
			"$$$/LrGeniusAI/LocalModel/Unsupported=This backend build has no local-model support; use Ollama or LM Studio."
		)
		propertyTable.llmDownloadChoices = {}
		-- Also clear the selection, not just the list: the Download button is
		-- gated on it, and a choice left over from an earlier refresh would
		-- otherwise stay clickable against a backend that cannot serve it.
		propertyTable.llmDownloadChoice = nil
		propertyTable.llmSupported = false
		return
	end
	propertyTable.llmSupported = true

	local names = {}
	for _, m in ipairs(catalog.installed or {}) do
		table.insert(names, m.name or "?")
	end
	propertyTable.llmInstalledText = #names > 0 and table.concat(names, ", ")
		or LOC("$$$/LrGeniusAI/LocalModel/NoneInstalled=none yet")

	local choices = toDownloadChoices(catalog.downloadable)
	propertyTable.llmDownloadChoices = choices
	propertyTable.llmDownloadChoice = pickValidChoice(choices, propertyTable.llmDownloadChoice)

	if status ~= nil and status.status == "loaded" then
		-- The per-photo window is what a user has to keep Max Tokens under, so
		-- that is the number to show; n_ctx alone reads as far more room than
		-- any one photo gets. Older backends do not send n_ctx_seq.
		propertyTable.llmStatusText = LOC(
			"$$$/LrGeniusAI/LocalModel/Loaded=Loaded: ^1 (^2 tokens per photo, ^3 parallel)",
			tostring(status.model_path or "?"),
			tostring(status.n_ctx_seq or status.n_ctx or "?"),
			tostring(status.n_parallel or "?")
		)
	elseif #names > 0 then
		propertyTable.llmStatusText =
			LOC("$$$/LrGeniusAI/LocalModel/ReadyNotLoaded=Installed and ready; loads on first use.")
	else
		propertyTable.llmStatusText = LOC("$$$/LrGeniusAI/LocalModel/NothingInstalled=No local model installed yet.")
	end
end

-- The MLX half of the same payload. Absent entirely when talking to an older
-- backend, which must read as "not available" rather than as an error.
local function updateMlxFields(propertyTable, catalog, status)
	local mlx = catalog.mlx
	if mlx == nil then
		propertyTable.mlxStatusText = LOC("$$$/LrGeniusAI/MlxModel/Unsupported=This backend does not support MLX.")
		propertyTable.mlxDownloadChoices = {}
		propertyTable.mlxDownloadChoice = nil
		propertyTable.mlxSupported = false
		return
	end
	if mlx.supported == false then
		-- The backend explains *why* (not Apple silicon, or the helper is
		-- missing); pass it through rather than inventing a reason.
		propertyTable.mlxStatusText = mlx.reason
			or LOC("$$$/LrGeniusAI/MlxModel/Unavailable=MLX is not available on this system.")
		propertyTable.mlxDownloadChoices = {}
		propertyTable.mlxDownloadChoice = nil
		propertyTable.mlxSupported = false
		return
	end
	propertyTable.mlxSupported = true

	local mlxNames = {}
	for _, m in ipairs(mlx.installed or {}) do
		table.insert(mlxNames, m.name or "?")
	end
	propertyTable.mlxInstalledText = #mlxNames > 0 and table.concat(mlxNames, ", ")
		or LOC("$$$/LrGeniusAI/MlxModel/NoneInstalled=none yet")

	local mlxChoices = toDownloadChoices(mlx.downloadable)
	propertyTable.mlxDownloadChoices = mlxChoices
	propertyTable.mlxDownloadChoice = pickValidChoice(mlxChoices, propertyTable.mlxDownloadChoice)

	local mlxStatus = status and status.mlx
	if mlxStatus ~= nil and mlxStatus.status == "loaded" then
		propertyTable.mlxStatusText =
			LOC("$$$/LrGeniusAI/MlxModel/Loaded=Loaded: ^1", tostring(mlxStatus.model_name or "?"))
	elseif #mlxNames > 0 then
		propertyTable.mlxStatusText =
			LOC("$$$/LrGeniusAI/MlxModel/ReadyNotLoaded=Installed and ready; loads on first use.")
	else
		propertyTable.mlxStatusText = LOC("$$$/LrGeniusAI/MlxModel/NothingInstalled=No MLX model installed yet.")
	end
end

---
-- Refreshes both local-model sections from /v1/llm/catalog and /v1/llm/status.
--
-- Everything here is best-effort: the backend may be down, or built without
-- local-model support, and neither should break the calling dialog. Runs in
-- its own async task, so callers may invoke it from any context.
--
function LocalModelCatalog.refresh(propertyTable)
	LrTasks.startAsyncTask(function()
		local catalog = SearchIndexAPI.getLlmCatalog()
		if catalog == nil then
			propertyTable.llmStatusText =
				LOC("$$$/LrGeniusAI/LocalModel/Unreachable=Backend not reachable — cannot list local models.")
			propertyTable.mlxStatusText =
				LOC("$$$/LrGeniusAI/MlxModel/Unreachable=Backend not reachable — cannot list MLX models.")
			propertyTable.llmSupported = false
			propertyTable.mlxSupported = false
			propertyTable.llmDownloadChoice = nil
			propertyTable.mlxDownloadChoice = nil
			return
		end

		local status = SearchIndexAPI.getLlmStatus()

		-- The two backends are independent: a build without the `llamacpp`
		-- feature still reports a usable MLX backend, so neither half may
		-- return out of the other's update.
		updateLlamaCppFields(propertyTable, catalog, status)
		updateMlxFields(propertyTable, catalog, status)
	end)
end

---
-- Starts the download of the model currently picked in one of the two sections.
--
-- Both engines share one download endpoint and one progress bar: the catalog id
-- alone picks the backend, so there is a single download queue rather than two
-- that could both claim the network.
--
-- @param propertyTable table Holds the `llmDownloadChoice`/`mlxDownloadChoice`.
-- @param kind string "llm" (llama.cpp/GGUF) or "mlx".
--
function LocalModelCatalog.startDownload(propertyTable, kind)
	local isMlx = kind == "mlx"
	local choice = isMlx and propertyTable.mlxDownloadChoice or propertyTable.llmDownloadChoice
	if not choice then
		return
	end

	LrTasks.startAsyncTask(function()
		local ok, err = SearchIndexAPI.startLlmDownload(choice)
		if not ok then
			ErrorHandler.handleError(
				isMlx and LOC("$$$/LrGeniusAI/MlxDownload/ErrorTitle=Error downloading MLX model")
					or LOC("$$$/LrGeniusAI/LlmDownload/ErrorTitle=Error downloading local AI model"),
				err
			)
			return
		end
		-- Reflect the "downloading" state right away; the periodic refresh in
		-- the calling dialog picks up completion.
		LocalModelCatalog.refresh(propertyTable)
	end)
end

return LocalModelCatalog
