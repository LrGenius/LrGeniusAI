-- Contract test: the develop settings tables the backend writes for
-- photo:applyDevelopSettings(), decoded the way the plugin decodes them.
--
-- server-rs/testdata/develop/wire/*.json are written by the backend's Lua
-- writer (lrg_develop::lua::to_lua_value, blessed with
-- `LRG_BLESS=1 cargo test -p lrg-develop --test lua_wire_goldens`). The Rust
-- tests pin the bytes; this spec checks what reaches Lightroom: the Lua table
-- the plugin's own JSON.lua makes of each file, under Lua 5.1 in CI as in
-- Lightroom.
--
-- The goldens are provisional until experiments E1, E2, E4 and E11
-- (TaskDevelopExperiments.lua) have been run. The forms those experiments
-- bear on (mask enums as numbers or numeric strings, 0/1 flags as numbers or
-- booleans, ...; see LuaOptions::PROVISIONAL in
-- server-rs/crates/lrg-develop/src/lua/write.rs) are accepted either way here.
-- What is checked holds for every outcome (plan section 7.3, point 5):
--   * real sequences (keys 1..n, nothing missing), no empty, mixed or null
--     value: JSON.lua encodes {} as [] and turns a null in a list into a hole,
--     and an empty MaskGroupBasedCorrections would delete every mask;
--   * every value has the Lua type of its key's registry class
--     (wire/key_classes.txt, generated from lrg-develop's registry next to
--     the goldens): numbers are numbers, numeric-looking strings stay
--     strings, compound strings hold numbers (ReferencePoint 2, LumRange and
--     FocalRange 4), booleans are booleans;
--   * global curves are flat number lists of even length >= 4, local curves
--     lists of "x,y" strings;
--   * every correction has a non-empty CorrectionMasks;
--   * no key Lightroom must not be handed (the registry's never-written
--     keys, digests, brush tables), no key the registry does not know, no
--     Temp at any depth, no ProcessVersion at the top;
--   * white balance as one family.
--
-- Values are compared decoded, never as encoded text: Lua 5.1 (Lightroom,
-- CI) re-encodes numbers with %.14g, a local Lua 5.5 with the shortest exact
-- form.
--
-- Run from the repo root with:  busted

local Json = require("JSON")
local lfs = require("lfs") -- a dependency of busted itself

local WIRE_DIR = "server-rs/testdata/develop/wire"
local KEY_CLASSES_FILE = WIRE_DIR .. "/key_classes.txt"

-- Decoding with a placeholder for null makes a null visible instead of a
-- missing key or a hole.
local NULL = setmetatable({}, {
	__tostring = function()
		return "JSON null"
	end,
})

local function decode(text)
	return Json:decode(text, nil, { null = NULL })
end

local function read(path)
	local fh = assert(io.open(path, "r"), "cannot open " .. path)
	local text = fh:read("*a")
	fh:close()
	return text
end

local function goldenFiles()
	local names = {}
	for name in lfs.dir(WIRE_DIR) do
		if name:match("%.json$") then
			names[#names + 1] = name
		end
	end
	table.sort(names)
	return names
end

local function set(list)
	local out = {}
	for _, k in ipairs(list) do
		out[k] = true
	end
	return out
end

-- `{ [key name] = { [class] = true } }` from wire/key_classes.txt: the
-- registry's classes of each key (a name at several levels has several),
-- or `never` for a key the writer never writes.
local function loadKeyClasses(path)
	local classes, count = {}, 0
	for line in read(path):gmatch("[^\r\n]+") do
		if line:sub(1, 1) ~= "#" then
			local name, rest = line:match("^(%S+)%s+(.+)$")
			assert(name, "malformed line in " .. path .. ": " .. line)
			local cs = {}
			for c in rest:gmatch("%S+") do
				cs[c] = true
			end
			classes[name] = cs
			count = count + 1
		end
	end
	return classes, count
end

local KEY_CLASSES, KEY_CLASS_COUNT = loadKeyClasses(KEY_CLASSES_FILE)

-- Numbers inside one string, per key that has a fixed count.
local COMPOUND_ARITY = { ReferencePoint = 2, LumRange = 4, FocalRange = 4 }
local SYNC_ID_KEYS = set({ "CorrectionSyncID", "MaskSyncID" })

local function isInteger(v)
	return type(v) == "number" and v == math.floor(v)
end

-- How many numbers `s` holds (separated by spaces or commas), or nil when
-- it holds none or anything else.
local function numbersIn(s)
	local count = 0
	for token in s:gmatch("[^%s,]+") do
		if tonumber(token) == nil then
			return nil
		end
		count = count + 1
	end
	return count > 0 and count or nil
end

-- Whether `v` is a valid compound number string for key `k`.
local function compoundOk(k, v)
	if type(v) ~= "string" then
		return false
	end
	local n = numbersIn(v)
	return n ~= nil and (COMPOUND_ARITY[k] == nil or n == COMPOUND_ARITY[k])
end

local function isList(v, item)
	if type(v) ~= "table" or #v == 0 then
		return false
	end
	for i = 1, #v do
		if not item(v[i]) then
			return false
		end
	end
	return true
end

-- One check per class in key_classes.txt.
local CLASS_CHECKS = {
	integer = isInteger,
	number = function(v)
		return type(v) == "number"
	end,
	-- 0/1 flags: numbers or booleans (LuaOptions.int_flag_as; no experiment
	-- writes a flag key, so the read-back form is the provisional one).
	flag = function(v)
		return type(v) == "boolean" or v == 0 or v == 1
	end,
	-- Mask enums (lrg_develop's KeySpec::is_mask_enum): integers, as numbers
	-- or numeric strings (LuaOptions.mask_enum_as; E2/E4 apply numbers).
	maskenum = function(v)
		return isInteger(v) or (type(v) == "string" and v:match("^%-?%d+$") ~= nil)
	end,
	boolean = function(v)
		return type(v) == "boolean"
	end,
	string = function(v)
		return type(v) == "string"
	end,
	compound = function(v, k)
		return compoundOk(k, v)
	end,
	strings = function(v)
		return isList(v, function(x)
			return type(x) == "string"
		end)
	end,
	compounds = function(v, k)
		return isList(v, function(x)
			return compoundOk(k, x)
		end)
	end,
	globalcurve = function(v)
		return type(v) == "table"
	end,
	localcurve = function(v)
		return type(v) == "table"
	end,
	table = function(v)
		return type(v) == "table"
	end,
	list = function(v)
		return type(v) == "table"
	end,
	any = function()
		return true
	end,
}

local function denied(key)
	local classes = KEY_CLASSES[key]
	return (classes ~= nil and classes.never)
		or key == "Temp"
		or key:find("Digest", 1, true) ~= nil
		or key:sub(1, 14) == "MaskBrushTable"
end

-- "object", "sequence", or what is wrong with the table.
local function classify(t)
	local n, strings, numbers = 0, 0, 0
	for k in pairs(t) do
		n = n + 1
		if type(k) == "string" then
			strings = strings + 1
		elseif type(k) == "number" then
			numbers = numbers + 1
		end
	end
	if n == 0 then
		return "empty"
	elseif strings == n then
		return "object"
	elseif numbers == n then
		for i = 1, n do
			if t[i] == nil then
				return "sparse"
			end
		end
		return "sequence"
	end
	return "mixed"
end

-- Calls visit(key, value, path, parentKey) for every keyed value below `t`
-- (list items are visited with their index as key; `parentKey` is the
-- nearest string key above).
local function walk(t, path, visit, parentKey)
	for k, v in pairs(t) do
		local p = path == "" and tostring(k)
			or (type(k) == "number" and (path .. "[" .. k .. "]") or (path .. "." .. k))
		visit(k, v, p, parentKey)
		if type(v) == "table" and v ~= NULL then
			walk(v, p, visit, type(k) == "string" and k or parentKey)
		end
	end
end

-- The violations of the structural rules, as a list of strings.
local function structureViolations(top)
	local bad = {}
	walk(top, "", function(_, v, path)
		if v == NULL then
			bad[#bad + 1] = path .. ": null"
		elseif type(v) == "table" then
			local kind = classify(v)
			if kind ~= "object" and kind ~= "sequence" then
				bad[#bad + 1] = path .. ": " .. kind .. " table"
			elseif kind == "sequence" then
				local first = type(v[1])
				for i = 2, #v do
					if type(v[i]) ~= first then
						bad[#bad + 1] = path .. ": a list mixing " .. first .. " and " .. type(v[i])
						break
					end
				end
			end
		end
	end)
	return bad
end

-- Every value against its key's registry classes, plus the keys that need
-- more than a type: sync ids, boolean-looking strings.
local function classViolations(top)
	local bad = {}
	walk(top, "", function(k, v, path, parentKey)
		if type(k) ~= "string" then
			return
		end
		if v == "true" or v == "false" then
			bad[#bad + 1] = path .. ": a boolean as a string"
		end
		if SYNC_ID_KEYS[k] and not (type(v) == "string" and v:match("^%x+$") and #v == 32) then
			bad[#bad + 1] = path .. ": not 32 hex digits"
		end
		local classes = KEY_CLASSES[k]
		if classes == nil then
			-- The one key that is no registry key: a text alternative's
			-- language (`Look.Group = { ["x-default"] = ... }`).
			if not (k == "x-default" and type(v) == "string") then
				bad[#bad + 1] = path .. ": not a key the registry knows"
			end
			return
		end
		if classes.never then
			return -- reported by the denylist check
		end
		-- `Type` is the range mask's kind, nothing else's.
		if k == "Type" and parentKey ~= "CorrectionRangeMask" then
			bad[#bad + 1] = path .. ": Type outside a CorrectionRangeMask"
			return
		end
		for class in pairs(classes) do
			local check = CLASS_CHECKS[class]
			assert(check, "key_classes.txt names an unknown class: " .. class)
			if check(v, k) then
				return
			end
		end
		local names = {}
		for class in pairs(classes) do
			names[#names + 1] = class
		end
		table.sort(names)
		bad[#bad + 1] = path .. ": " .. type(v) .. " " .. tostring(v) .. " is not " .. table.concat(names, " or ")
	end)
	return bad
end

-- The curve shapes (rule 5).
local function curveViolations(top)
	local bad = {}
	walk(top, "", function(k, v, path)
		local classes = type(k) == "string" and KEY_CLASSES[k]
		if not classes then
			return
		end
		if classes.globalcurve then
			if type(v) ~= "table" or #v < 4 or #v % 2 ~= 0 then
				bad[#bad + 1] = path .. ": not an even list of at least 4"
			else
				for i = 1, #v do
					if type(v[i]) ~= "number" then
						bad[#bad + 1] = path .. "[" .. i .. "]: " .. type(v[i])
					end
				end
			end
		elseif classes.localcurve then
			if type(v) ~= "table" or #v < 2 then
				bad[#bad + 1] = path .. ": not a list of at least 2 points"
			else
				for i = 1, #v do
					if not (type(v[i]) == "string" and numbersIn(v[i]) == 2 and v[i]:find(",", 1, true)) then
						bad[#bad + 1] = path .. "[" .. i .. "]: not an x,y string"
					end
				end
			end
		end
	end)
	return bad
end

-- What a golden holds, counted, so a re-blessed set that lost a shape
-- cannot make the checks above pass vacuously.
local function countShapes(top, seen)
	if type(top) ~= "table" then
		return
	end
	if type(top.MaskGroupBasedCorrections) == "table" then
		seen.corrections = seen.corrections + #top.MaskGroupBasedCorrections
	end
	walk(top, "", function(k, v)
		local classes = type(k) == "string" and KEY_CLASSES[k]
		if not classes then
			return
		end
		if classes.globalcurve then
			seen.globalCurves = seen.globalCurves + 1
		end
		if classes.localcurve then
			seen.localCurves = seen.localCurves + 1
		end
		if classes.compound and type(v) == "string" then
			seen.compoundStrings = seen.compoundStrings + 1
		end
		if classes.compounds and type(v) == "table" then
			seen.compoundLists = seen.compoundLists + 1
		end
		if classes.boolean and type(v) == "boolean" then
			seen.booleans = seen.booleans + 1
		end
		if classes.flag then
			seen.flags = seen.flags + 1
		end
		if classes.maskenum then
			seen.maskEnums = seen.maskEnums + 1
		end
		if classes.number and type(v) == "number" then
			seen.numbers = seen.numbers + 1
		end
	end)
end

local function close(a, b)
	return a == b or math.abs(a - b) <= 1e-9 * math.max(1, math.abs(a), math.abs(b))
end

-- Deep equality of decoded values, numbers within the precision %.14g keeps.
local function same(a, b)
	if type(a) == "number" and type(b) == "number" then
		return close(a, b)
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in pairs(a) do
		if not same(v, b[k]) then
			return false
		end
	end
	for k in pairs(b) do
		if a[k] == nil then
			return false
		end
	end
	return true
end

describe("JSON.lua, as the wire format relies on it", function()
	it("encodes an empty table as an empty JSON array", function()
		-- So an empty object would arrive as an absent key, and an empty
		-- MaskGroupBasedCorrections as []: the writer never sends either.
		assert.are.equal("[]", Json:encode({}))
	end)

	it("decodes a null inside an array to a hole", function()
		local t = Json:decode("[1,null,3]")
		assert.are.equal(1, t[1])
		assert.is_nil(t[2])
		assert.are.equal(3, t[3])
		assert.are.equal("sparse", classify(t))
	end)

	it("keeps numeric-looking strings as strings", function()
		local t = Json:decode('{"ProcessVersion":"15.4","ReferencePoint":"0.5 0.5","n":15.4}')
		assert.are.equal("string", type(t.ProcessVersion))
		assert.are.equal("15.4", t.ProcessVersion)
		assert.are.equal("string", type(t.ReferencePoint))
		assert.are.equal("number", type(t.n))
	end)
end)

describe("the wire checks themselves", function()
	-- Each broken table must be caught, so a check that silently passes
	-- everything fails here instead.
	local function violations(top)
		local bad = {}
		for _, list in ipairs({ structureViolations(top), classViolations(top), curveViolations(top) }) do
			for _, b in ipairs(list) do
				bad[#bad + 1] = b
			end
		end
		return bad
	end

	it("know the registry's key classes", function()
		assert.is_true(KEY_CLASS_COUNT > 300, "only " .. KEY_CLASS_COUNT .. " keys in " .. KEY_CLASSES_FILE)
		assert.is_true(KEY_CLASSES.Exposure2012.number)
		assert.is_true(KEY_CLASSES.MaskSubType.maskenum)
		assert.is_true(KEY_CLASSES.ErrorReason.maskenum)
		assert.is_true(KEY_CLASSES.Type.maskenum)
		assert.is_true(KEY_CLASSES.CorrectionID.never)
	end)

	it("count no number in an empty or blank string", function()
		assert.is_nil(numbersIn(""))
		assert.is_nil(numbersIn("  "))
		assert.are.equal(2, numbersIn("0.5 0.5"))
		assert.is_nil(numbersIn("0.5 x"))
	end)

	for what, broken in pairs({
		["a number as a string"] = { Exposure2012 = "0.5" },
		["an empty string on a number key"] = { Exposure2012 = "" },
		["text on a number key"] = { Contrast2012 = "x" },
		["an empty compound string"] = {
			MaskGroupBasedCorrections = {
				{
					What = "Correction",
					CorrectionMasks = { { What = "Mask/Image", ReferencePoint = "" } },
				},
			},
		},
		["a compound string with the wrong count"] = {
			MaskGroupBasedCorrections = {
				{
					What = "Correction",
					CorrectionMasks = { { What = "Mask/Image", ReferencePoint = "0.5" } },
				},
			},
		},
		["a mask enum that is no integer"] = {
			MaskGroupBasedCorrections = {
				{
					What = "Correction",
					CorrectionMasks = { { What = "Mask/Image", ErrorReason = "zero" } },
				},
			},
		},
		["a range kind that is no integer"] = {
			MaskGroupBasedCorrections = {
				{
					What = "Correction",
					CorrectionMasks = { { What = "Mask/RangeMask", CorrectionRangeMask = { Type = "x" } } },
				},
			},
		},
		["a boolean as a number"] = { ConvertToGrayscale = 1 },
		["a key the registry does not know"] = { NotADevelopKey = 1 },
		["an odd global curve"] = { ToneCurvePV2012 = { 0, 0, 255 } },
	}) do
		it("catch " .. what, function()
			assert.is_true(#violations(broken) > 0, what)
		end)
	end

	it("accept both forms of each provisional encoding", function()
		local masks = {
			{
				What = "Correction",
				CorrectionMasks = {
					{ What = "Mask/Image", MaskSubType = "1", MaskBlendMode = 0, ErrorReason = "0" },
					{ What = "Mask/RangeMask", CorrectionRangeMask = { Type = "2", SampleType = 0 } },
				},
			},
		}
		assert.are.same(
			{},
			violations({ LensProfileEnable = true, AutoLateralCA = 1, MaskGroupBasedCorrections = masks })
		)
	end)
end)

describe("native wire format goldens", function()
	local names = goldenFiles()
	local goldens = {}
	-- Counted here, while collecting, so the coverage check below depends on
	-- no other test having run (busted --shuffle, --filter).
	local seen = {
		corrections = 0,
		globalCurves = 0,
		localCurves = 0,
		compoundStrings = 0,
		compoundLists = 0,
		booleans = 0,
		flags = 0,
		maskEnums = 0,
		numbers = 0,
	}
	for _, name in ipairs(names) do
		local top = decode(read(WIRE_DIR .. "/" .. name))
		goldens[#goldens + 1] = { name = name, top = top }
		countShapes(top, seen)
	end

	it("are present", function()
		-- An empty or moved folder would make every check below vacuous.
		assert.is_true(#names >= 18, "only " .. #names .. " goldens in " .. WIRE_DIR)
	end)

	it("cover every shape the checks are about", function()
		for what, count in pairs(seen) do
			assert.is_true(count > 0, "no golden has " .. what)
		end
	end)

	for _, golden in ipairs(goldens) do
		local name, top = golden.name, golden.top

		describe(name, function()
			it("is a non-empty table with string keys at the top", function()
				assert.are.equal("table", type(top))
				assert.are.equal("object", classify(top))
			end)

			it("holds real sequences and no empty, mixed, sparse or null value", function()
				assert.are.same({}, structureViolations(top))
			end)

			it("gives every value the type of its key's registry class", function()
				assert.are.same({}, classViolations(top))
			end)

			it("writes global curves as flat number lists and local curves as x,y strings", function()
				assert.are.same({}, curveViolations(top))
			end)

			it("gives every correction a non-empty CorrectionMasks", function()
				local corrections = top.MaskGroupBasedCorrections
				if corrections == nil then
					return
				end
				assert.are.equal("sequence", classify(corrections))
				for i, c in ipairs(corrections) do
					local where = "MaskGroupBasedCorrections[" .. i .. "]"
					assert.are.equal("Correction", c.What, where)
					assert.are.equal("table", type(c.CorrectionMasks), where)
					assert.are.equal("sequence", classify(c.CorrectionMasks), where)
					for j, m in ipairs(c.CorrectionMasks) do
						assert.is_truthy(
							type(m.What) == "string" and m.What:match("^Mask/"),
							where .. ".CorrectionMasks[" .. j .. "]"
						)
					end
				end
			end)

			it("carries no key Lightroom must not be handed, no Temp and no ProcessVersion", function()
				local bad = {}
				walk(top, "", function(k, _, path)
					if type(k) == "string" and denied(k) then
						bad[#bad + 1] = path
					end
				end)
				assert.are.same({}, bad)
				-- The example's process version describes the example, not the photo.
				assert.is_nil(top.ProcessVersion)
			end)

			it("writes white balance as one family", function()
				local kelvin = top.Temperature ~= nil or top.Tint ~= nil
				local incremental = top.IncrementalTemperature ~= nil or top.IncrementalTint ~= nil
				assert.is_false(kelvin and incremental, "both white-balance families")
				if kelvin or incremental then
					-- Numbers only next to Custom, or with no mode at all
					-- (LuaOptions.wb_custom_with_numbers).
					assert.is_truthy(top.WhiteBalance == nil or top.WhiteBalance == "Custom")
				end
			end)

			it("decodes to the same values after the plugin's own encode", function()
				assert.is_true(same(top, decode(Json:encode(top))))
			end)
		end)
	end
end)
