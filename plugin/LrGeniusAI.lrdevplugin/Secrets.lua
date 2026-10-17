---
-- The API keys: OpenAI, Google Gemini, Anthropic and the Other AI server.
--
-- They used to be plug-in prefs, which Lightroom writes to disk as plain text
-- (com.adobe.LightroomClassicCC7.plist on macOS, the .agprefs file on
-- Windows) where any program running as the user can read them. They now live
-- in the password storage Lightroom gives plug-ins (LrPasswords), which uses
-- the operating system's own and is scoped to this plug-in's ID.
--
-- Read a key through Secrets.get, never from prefs: once moved, it is gone
-- from prefs. Each key is read from the storage once per Lightroom session.
--
-- The SDK does not say whether LrPasswords may be called outside an async
-- task, so everything that writes runs in one (see runInTask), and Init.lua
-- fills the cache from a task at startup.
--
Secrets = {}

--- The keys, by the name of the pref each used to be. The name is also the
--- LrPasswords key string, so it must never change.
Secrets.FIELDS = { "chatgptApiKey", "geminiApiKey", "anthropicApiKey", "aiServerApiKey" }

local LABELS = {
	chatgptApiKey = "OpenAI API key",
	geminiApiKey = "Google Gemini API key",
	anthropicApiKey = "Anthropic API key",
	aiServerApiKey = "Other AI server API key",
}

--- Prefs that held a key in an early version and have been unused since.
--- Removed, not moved.
local RETIRED_FIELDS = { "apiKey" }

--- The password storage. Specs swap in a double.
Secrets.storage = import("LrPasswords")

-- field -> key ("" for none). A field is absent until read successfully.
local cache = {}
-- What went wrong this session, for the Plug-in Manager's status line.
local problems = {}

local function blank(value)
	return value == nil or (type(value) == "string" and value:match("^%s*$") ~= nil)
end

local function trim(value)
	return (tostring(value or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

local function addProblem(text)
	for _, existing in ipairs(problems) do
		if existing == text then
			return
		end
	end
	table.insert(problems, text)
end

local function describe(err)
	local text = trim(err)
	return text ~= "" and text or "no reason given"
end

local function retrieve(field)
	local ok, value = LrTasks.pcall(Secrets.storage.retrieve, field)
	if not ok then
		return nil, describe(value)
	end
	return trim(value)
end

-- Stores `value` and reads it back: a key counts as stored only once the
-- storage returns it unchanged.
local function storeVerified(field, value)
	local ok, err = LrTasks.pcall(Secrets.storage.store, field, value)
	if not ok then
		return false, describe(err)
	end
	local readBack, readErr = retrieve(field)
	if readBack == nil then
		return false, readErr
	end
	if readBack ~= value then
		return false, "the key read back from the password storage did not match"
	end
	return true
end

local function runInTask(fn)
	if LrTasks.canYield() then
		fn()
	else
		LrTasks.startAsyncTask(fn)
	end
end

---
-- A key, or "" when none is set.
--
-- @param field string One of Secrets.FIELDS.
-- @return string The key, trimmed.
-- @return string|nil Why the key could not be read; the text says what to do.
--
function Secrets.get(field)
	if cache[field] ~= nil then
		return cache[field]
	end
	local value, err = retrieve(field)
	if not blank(value) then
		cache[field] = value
		return value
	end
	-- Not moved yet (this version has not started before, or the move
	-- failed and left the key where it was): prefs still hold it. Not cached:
	-- the key belongs in the storage, and Secrets.migrate puts it there.
	local legacy = prefs and prefs[field]
	if not blank(legacy) then
		return trim(legacy)
	end
	if value == nil then
		local problem = "Your "
			.. LABELS[field]
			.. " could not be read from the system's password storage ("
			.. err
			.. "). Enter it again under Plug-in Manager → Optional AI providers."
		addProblem(problem)
		return "", problem
	end
	cache[field] = ""
	return ""
end

---
-- Whether a key is set.
--
-- @param field string One of Secrets.FIELDS.
-- @return boolean
--
function Secrets.has(field)
	return not blank(Secrets.get(field))
end

---
-- Saves a key, or removes it when `value` is blank. Must run in a task.
--
-- Takes effect for this session even when storing fails. A key that cannot be
-- stored is kept in prefs as before, so it still works next session; the
-- returned text tells the user it is there.
--
-- @param field string One of Secrets.FIELDS.
-- @param value string|nil
-- @return boolean Whether the key is in the password storage.
-- @return string|nil What went wrong, for the user.
--
function Secrets.set(field, value)
	value = trim(value)
	cache[field] = value
	local ok, err = storeVerified(field, value)
	if ok then
		prefs[field] = nil
		return true
	end
	log:error("Storing the " .. LABELS[field] .. " failed: " .. err)
	if value == "" then
		prefs[field] = nil
		return false,
			"Your "
				.. LABELS[field]
				.. " could not be removed from the system's password storage ("
				.. err
				.. "). It will be back after Lightroom restarts."
	end
	prefs[field] = value
	return false,
		"Your "
			.. LABELS[field]
			.. " could not be saved in the system's password storage ("
			.. err
			.. "). It was saved in Lightroom's preferences instead, as plain text."
end

---
-- Moves the keys still in prefs into the password storage, and fills the
-- cache. Runs at every start, not once: an older version of the plug-in,
-- after going back to it, writes its keys to prefs again. Must run in a task.
--
-- A key leaves prefs only after the storage has returned it unchanged.
--
-- @return table Problems, as text for the user; empty when all went well.
--   Not logged here: the caller does that.
--
function Secrets.migrate()
	local found = {}
	for _, field in ipairs(Secrets.FIELDS) do
		local legacy = prefs[field]
		if blank(legacy) then
			-- "" was the default these prefs were given at startup.
			prefs[field] = nil
		else
			local value = trim(legacy)
			local ok, err = storeVerified(field, value)
			if ok then
				cache[field] = value
				prefs[field] = nil
				log:info("Moved the " .. LABELS[field] .. " to the system's password storage.")
			else
				local problem = "Your "
					.. LABELS[field]
					.. " could not be moved to the system's password storage ("
					.. err
					.. "). It still works, but stays in Lightroom's preferences as plain text."
				addProblem(problem)
				table.insert(found, problem)
			end
		end
		local _, problem = Secrets.get(field)
		if problem then
			table.insert(found, problem)
		end
	end
	for _, field in ipairs(RETIRED_FIELDS) do
		prefs[field] = nil
	end
	return found
end

---
-- Copies keys into a dialog's property table.
--
-- @param propertyTable table
-- @param fields table|nil Which of Secrets.FIELDS; all when nil.
-- @return table What was loaded, for Secrets.saveChanged.
--
function Secrets.loadInto(propertyTable, fields)
	local loaded = {}
	for _, field in ipairs(fields or Secrets.FIELDS) do
		loaded[field] = Secrets.get(field)
		propertyTable[field] = loaded[field]
	end
	return loaded
end

---
-- Saves the keys the user changed in a dialog, and tells the user about any
-- that could not be stored.
--
-- Only changed keys: a key that could not be read shows as empty, and saving
-- that unchanged "" would delete the key the user still has.
--
-- @param propertyTable table
-- @param loaded table What Secrets.loadInto returned for this dialog.
--
function Secrets.saveChanged(propertyTable, loaded)
	local changed = {}
	for field, before in pairs(loaded) do
		local value = trim(propertyTable[field])
		if value ~= before then
			changed[field] = value
			-- Now, not when the task below runs: a task started right after
			-- the dialog closes must already see the new key.
			cache[field] = value
		end
	end
	if next(changed) == nil then
		return
	end
	runInTask(function()
		local failed = {}
		for _, field in ipairs(Secrets.FIELDS) do
			if changed[field] ~= nil then
				local ok, problem = Secrets.set(field, changed[field])
				if not ok then
					table.insert(failed, problem)
				end
			end
		end
		if #failed > 0 then
			LrDialogs.message("API keys not stored securely", table.concat(failed, "\n\n"), "warning")
		end
	end)
end

---
-- The fields AiProviders.connectionOptions reads: the Other AI server address
-- from prefs, and the keys.
--
-- @param p table The plug-in prefs.
-- @return table
--
function Secrets.connectionFields(p)
	local fields = { aiServerUrl = p and p.aiServerUrl, keyProblems = {} }
	for _, field in ipairs(Secrets.FIELDS) do
		local value, problem = Secrets.get(field)
		fields[field] = value
		fields.keyProblems[field] = problem
	end
	return fields
end

---
-- What went wrong with the keys this session, as text for the user.
--
-- @return table
--
function Secrets.problems()
	local copy = {}
	for i, text in ipairs(problems) do
		copy[i] = text
	end
	return copy
end

--- Forgets the cache and the problems. For specs.
function Secrets._reset()
	cache = {}
	problems = {}
end

return Secrets
