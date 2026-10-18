-- Every field that shows an API key is a password_field, so a key is never on
-- screen in plain text — in a screenshot, a screen share or over a shoulder.
--
-- Scans the plug-in's Lua files for bindings to the keys in Secrets.FIELDS —
-- `bind("chatgptApiKey")` or `key = "chatgptApiKey"` — and checks that the
-- view each one belongs to, the last factory call before it, is
-- f:password_field. A key added to Secrets.FIELDS is covered without a new line.
--
-- Run from the repo root with:  busted

local lfs = require("lfs")
local Secrets = require("Secrets")

local PLUGIN_DIR = "plugin/LrGeniusAI.lrdevplugin"

local function read(path)
	local fh = assert(io.open(path, "r"), "cannot open " .. path)
	local text = fh:read("*a")
	fh:close()
	return text
end

local function lineAt(text, pos)
	local _, newlines = text:sub(1, pos):gsub("\n", "")
	return newlines + 1
end

-- The factory method of the view whose table contains `pos`: the last
-- `:name({` before it.
local function viewAt(text, pos)
	local view
	for name in text:sub(1, pos):gmatch(":([%a_][%w_]*)%s*%(%s*{") do
		view = name
	end
	return view
end

describe("API key fields", function()
	it("are password fields wherever a key is bound", function()
		local isSecret = {}
		for _, field in ipairs(Secrets.FIELDS) do
			isSecret[field] = true
		end

		local bindings, plainText = 0, {}
		for file in lfs.dir(PLUGIN_DIR) do
			if file:match("%.lua$") then
				local text = read(PLUGIN_DIR .. "/" .. file)
				for _, pattern in ipairs({ 'bind%s*%(%s*"([%w_]+)"()', 'key%s*=%s*"([%w_]+)"()' }) do
					for name, pos in text:gmatch(pattern) do
						if isSecret[name] then
							bindings = bindings + 1
							local view = viewAt(text, pos)
							if view ~= "password_field" then
								table.insert(
									plainText,
									string.format("%s:%d %s in f:%s", file, lineAt(text, pos), name, tostring(view))
								)
							end
						end
					end
				end
			end
		end
		table.sort(plainText)

		assert.is_true(bindings > 0, "no bindings to an API key found — did the dialogs' binding style change?")
		assert.same({}, plainText, "API keys shown in a view other than f:password_field")
	end)
end)
