-- Headless test harness for the Lightroom plugin.
--
-- Lightroom plugin modules normally run inside Lightroom, where the SDK injects
-- a set of globals (LOC, import, log, prefs, the Lr* namespaces, ...). To unit
-- test the *pure* logic in modules like Util.lua outside of Lightroom, we stub
-- just enough of that environment here so the modules can be `require`d.
--
-- This helper is loaded by busted (see /.busted) before any spec runs.

-- LOC() mirrors what Lightroom actually does: strip the "$$$/path=" key
-- prefix and substitute ^1..^9 with the trailing arguments.
--
-- It used to return the raw key string unchanged, which quietly made every
-- assertion about user-facing text meaningless — a test could assert on
-- "$$$/Foo/Bar=..." and pass while the real dialog said something else.
_G.LOC = function(s, ...)
	if type(s) ~= "string" then
		return s
	end
	local text = s:match("^%$%$%$/[^=]*=(.*)$") or s
	local args = { ... }
	text = text:gsub("%^(%d)", function(digit)
		local value = args[tonumber(digit)]
		if value == nil then
			return "^" .. digit
		end
		return tostring(value)
	end)
	return text
end

-- import("LrFoo") returns a no-op stand-in. Any field access yields a function
-- that swallows its arguments, so incidental SDK calls don't blow up. Tests that
-- need real behaviour should inject their own doubles.
local noop = function() end
local function stub_namespace()
	return setmetatable({}, {
		__index = function()
			return noop
		end,
		__call = function()
			return noop
		end,
	})
end

_G.import = function()
	return stub_namespace()
end

_G._PLUGIN = { id = "com.lrgeniusai.test", path = "." }
_G.MAC_ENV = true
_G.WIN_ENV = false
_G.prefs = {}

-- log:error / log:info / log:trace ... all become no-ops.
_G.log = stub_namespace()

-- A few Lr* namespaces are referenced at the top level of some modules; provide
-- harmless stand-ins so requiring them never fails.
for _, name in ipairs({
	"LrApplication",
	"LrPathUtils",
	"LrFileUtils",
	"LrDate",
	"LrStringUtils",
	"LrMD5",
	"LrTasks",
}) do
	_G[name] = stub_namespace()
end

-- A real implementation, not a no-op: code under test compares trimmed strings,
-- and a stub returning nil would make such a comparison pass for the wrong
-- reason. Matches the SDK's documented behaviour (leading/trailing whitespace).
_G.LrStringUtils.trimWhitespace = function(s)
	if type(s) ~= "string" then
		return s
	end
	return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

-- LrTasks.pcall is the SDK's yield-safe pcall, and the plugin uses it wherever
-- a catalog call might fail. The no-op stub made every such call look like a
-- failure, so give it the real semantics: outside a Lightroom task the only
-- difference from Lua's pcall is that yielding is impossible anyway.
_G.LrTasks.pcall = function(fn, ...)
	return pcall(fn, ...)
end

-- Init.lua requires Util before ErrorHandler and APISearchIndex, so at runtime
-- the `Util` global those modules call into is always in place. Specs require
-- modules one at a time, in whatever order busted loads the files, so establish
-- it here instead of making each spec remember to.
require("Util")
