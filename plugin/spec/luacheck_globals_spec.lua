-- Keeps .luacheckrc's read_globals in lockstep with the SDK namespaces Init.lua
-- puts on _G. Both directions matter:
--   * imported but undeclared -> luacheck fails at the first bare use, pointing
--     at the call site instead of at the missing .luacheckrc entry. This is how
--     LrErrors, LrPrefs and LrSystemInfo sat undeclared for as long as no new
--     file happened to touch them.
--   * declared but not imported -> luacheck green-lights an access that is nil
--     at runtime, which is the worse direction.
--
-- Note the `Lr%w+` pattern rather than `Lr%a+`: LrMD5 ends in a digit, and a
-- letters-only pattern silently drops it from one side of the comparison and
-- invents a mismatch.
--
-- If a namespace is ever imported file-locally (`local LrFoo = import("LrFoo")`)
-- instead of via Init.lua, the fix is to DROP its read_globals entry, not to add
-- a token _G import here.
--
-- Run from the repo root with:  busted

local function read(path)
	local fh = assert(io.open(path, "r"), "cannot open " .. path)
	local text = fh:read("*a")
	fh:close()
	return text
end

describe("luacheck globals", function()
	it("declares exactly the Lr namespaces Init.lua puts on _G", function()
		local imported = {}
		for name in read("plugin/LrGeniusAI.lrdevplugin/Init.lua"):gmatch("_G%.(Lr%w+)%s*=%s*import") do
			imported[name] = true
		end
		assert.is_truthy(next(imported), "no _G.Lr* imports found — did Init.lua's import style change?")

		local declared = {}
		local readGlobals = read(".luacheckrc"):match("read_globals%s*=%s*{(.-)}")
		assert.is_truthy(readGlobals, "read_globals block not found in .luacheckrc")
		for name in readGlobals:gmatch('"(Lr%w+)"') do
			declared[name] = true
		end

		local missing, extra = {}, {}
		for name in pairs(imported) do
			if not declared[name] then
				table.insert(missing, name)
			end
		end
		for name in pairs(declared) do
			if not imported[name] then
				table.insert(extra, name)
			end
		end
		table.sort(missing)
		table.sort(extra)

		assert.same({}, missing, "imported in Init.lua but missing from .luacheckrc read_globals")
		assert.same({}, extra, "declared in .luacheckrc but never imported in Init.lua")
	end)
end)
