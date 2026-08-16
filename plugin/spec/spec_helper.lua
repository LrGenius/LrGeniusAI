-- Headless test harness for the Lightroom plugin.
--
-- Lightroom plugin modules normally run inside Lightroom, where the SDK injects
-- a set of globals (LOC, import, log, prefs, the Lr* namespaces, ...). To unit
-- test the *pure* logic in modules like Util.lua outside of Lightroom, we stub
-- just enough of that environment here so the modules can be `require`d.
--
-- This helper is loaded by busted (see /.busted) before any spec runs.

-- LOC() in tests just returns the key/default string unchanged.
_G.LOC = function(s, ...)
	return s
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
