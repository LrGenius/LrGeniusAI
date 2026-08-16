-- Advanced unit tests for pure-logic helpers in Util.lua.
--
-- Covers: dumpTable, extractAllKeywords, buildHierarchyFromPaths,
-- keywordTableToHierarchyStringList, keywordTableToStringList,
-- and nilOrEmpty (additional cases).
--
-- Run from the repo root with:  busted

local Util = require("Util")

-- ---------------------------------------------------------------------------
-- Helpers
-- ---------------------------------------------------------------------------

--- Split a semicolon-separated string into a sorted list of non-empty parts.
-- Order-insensitive assertions are made by sorting the result.
local function splitSemicolon(s)
	local parts = {}
	for part in s:gmatch("([^;]+)") do
		table.insert(parts, part)
	end
	table.sort(parts)
	return parts
end

--- Collect all keyword *names* from (vals, orderedIds) into a sorted list.
local function sortedNames(vals, ids)
	local names = {}
	for _, id in ipairs(ids) do
		names[#names + 1] = vals[id]
	end
	table.sort(names)
	return names
end

-- ---------------------------------------------------------------------------
-- Util.dumpTable
-- ---------------------------------------------------------------------------

describe("Util.dumpTable", function()
	it("returns '{}' for an empty table", function()
		assert.are.equal("{}", Util.dumpTable({}))
	end)

	it("serialises a simple array and contains all entries", function()
		local s = Util.dumpTable({ "alpha", "beta", "gamma" })
		assert.truthy(s:find("alpha", 1, true))
		assert.truthy(s:find("beta", 1, true))
		assert.truthy(s:find("gamma", 1, true))
	end)

	it("preserves array entry order", function()
		local s = Util.dumpTable({ "first", "second", "third" })
		local p1 = s:find("first", 1, true)
		local p2 = s:find("second", 1, true)
		local p3 = s:find("third", 1, true)
		assert.truthy(p1)
		assert.truthy(p2)
		assert.truthy(p3)
		assert.is_true(p1 < p2)
		assert.is_true(p2 < p3)
	end)

	it("serialises a hash table, including key and value", function()
		local s = Util.dumpTable({ fruit = "apple" })
		assert.truthy(s:find("fruit", 1, true))
		assert.truthy(s:find("apple", 1, true))
	end)

	it("serialises a nested table and includes nested keys and values", function()
		local s = Util.dumpTable({ outer = { inner = "value" } })
		assert.truthy(s:find("outer", 1, true))
		assert.truthy(s:find("inner", 1, true))
		assert.truthy(s:find("value", 1, true))
	end)

	it("handles a cyclic reference without infinite recursion", function()
		local t = {}
		t.self = t
		-- Must not blow the stack; result must contain the cycle sentinel.
		local ok, s = pcall(Util.dumpTable, t)
		assert.is_true(ok)
		-- The implementation emits "{ ...cycle... }"
		assert.truthy(s:find("cycle", 1, true))
	end)

	it("redacts the api_key field value", function()
		local s = Util.dumpTable({ api_key = "supersecret" })
		-- The raw secret must not appear.
		assert.is_nil(s:find("supersecret", 1, true))
		-- The redaction marker must be present.
		assert.truthy(s:find("<redacted>", 1, true))
		-- The field name itself is still visible.
		assert.truthy(s:find("api_key", 1, true))
	end)

	it("redacts the chatgptApiKey field value", function()
		local s = Util.dumpTable({ chatgptApiKey = "sk-abc123" })
		assert.is_nil(s:find("sk-abc123", 1, true))
		assert.truthy(s:find("<redacted>", 1, true))
	end)

	it("redacts the geminiApiKey field value", function()
		local s = Util.dumpTable({ geminiApiKey = "AIzaSy_fakekey" })
		assert.is_nil(s:find("AIzaSy_fakekey", 1, true))
		assert.truthy(s:find("<redacted>", 1, true))
	end)

	it("replaces a base64 payload in the 'data' field", function()
		-- "SGVsbG8gV29ybGQ=" is base64("Hello World")
		local s = Util.dumpTable({ data = "SGVsbG8gV29ybGQ=" })
		assert.is_nil(s:find("SGVsbG8gV29ybGQ=", 1, true))
		assert.truthy(s:find("base64 removed", 1, true))
	end)

	it("replaces a data-URI base64 value in the 'url' field", function()
		local s = Util.dumpTable({ url = "data:image/jpeg;base64,/9j/4AAQSkZJRgAB==" })
		assert.is_nil(s:find("/9j/4AAQSkZJRgAB==", 1, true))
		assert.truthy(s:find("base64 removed", 1, true))
	end)

	it("does not redact unrelated fields", function()
		local s = Util.dumpTable({ username = "alice", score = 42 })
		assert.truthy(s:find("alice", 1, true))
		assert.truthy(s:find("42", 1, true))
	end)

	it("renders boolean values as 'true' / 'false'", function()
		local s = Util.dumpTable({ yes = true, no = false })
		assert.truthy(s:find("true", 1, true))
		assert.truthy(s:find("false", 1, true))
	end)

	it("renders number values", function()
		local s = Util.dumpTable({ count = 99 })
		assert.truthy(s:find("99", 1, true))
	end)

	-- Hash keys that are not valid Lua identifiers are wrapped in ["..."].
	it("wraps non-identifier string keys in brackets", function()
		local s = Util.dumpTable({ ["key-with-dash"] = "v" })
		assert.truthy(s:find('["key-with-dash"]', 1, true))
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.nilOrEmpty — additional edge cases
-- ---------------------------------------------------------------------------

describe("Util.nilOrEmpty (additional cases)", function()
	it("returns false for number 0", function()
		assert.is_false(Util.nilOrEmpty(0))
	end)

	it("returns false for number 1", function()
		assert.is_false(Util.nilOrEmpty(1))
	end)

	it("returns false for boolean false", function()
		assert.is_false(Util.nilOrEmpty(false))
	end)

	it("returns false for boolean true", function()
		assert.is_false(Util.nilOrEmpty(true))
	end)

	it("returns false for a non-empty table", function()
		assert.is_false(Util.nilOrEmpty({ 1 }))
	end)

	it("returns false for an empty table (not nil, not a string)", function()
		-- The implementation only returns true for nil or whitespace-only strings;
		-- an empty table is neither.
		assert.is_false(Util.nilOrEmpty({}))
	end)

	it("returns true for a string containing only tabs and newlines", function()
		assert.is_true(Util.nilOrEmpty("\t\n  \t"))
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.extractAllKeywords
-- ---------------------------------------------------------------------------

describe("Util.extractAllKeywords", function()
	it("returns three empty tables for nil input", function()
		local vals, meta, ids = Util.extractAllKeywords(nil)
		assert.are.same({}, vals)
		assert.are.same({}, meta)
		assert.are.same({}, ids)
	end)

	it("returns three empty tables for an empty table", function()
		local vals, meta, ids = Util.extractAllKeywords({})
		assert.are.same({}, vals)
		assert.are.same({}, meta)
		assert.are.same({}, ids)
	end)

	it("returns three empty tables for a non-table (string) argument", function()
		local vals, meta, ids = Util.extractAllKeywords("not a table")
		assert.are.same({}, vals)
		assert.are.same({}, meta)
		assert.are.same({}, ids)
	end)

	it("extracts a flat array of string keywords", function()
		local vals, _, ids = Util.extractAllKeywords({ "Cat", "Dog" })
		assert.are.equal(2, #ids)
		assert.are.same({ "Cat", "Dog" }, sortedNames(vals, ids))
	end)

	it("assigns sequential IDs in the kw_N pattern", function()
		local _, _, ids = Util.extractAllKeywords({ "Alpha", "Beta" })
		assert.are.equal(2, #ids)
		assert.are.equal("kw_1", ids[1])
		assert.are.equal("kw_2", ids[2])
	end)

	it("extracts keywords nested under a string category key", function()
		local vals, meta, ids = Util.extractAllKeywords({ Animals = { "Cat", "Dog" } })
		assert.are.equal(2, #ids)
		assert.are.same({ "Cat", "Dog" }, sortedNames(vals, ids))
		-- Each keyword's meta path must reference the parent category.
		for _, id in ipairs(ids) do
			assert.truthy(meta[id].path:find("Animals", 1, true))
		end
	end)

	it("records the full path for a deeply nested keyword", function()
		local _, meta, ids = Util.extractAllKeywords({ Animals = { Birds = { "Owl" } } })
		assert.are.equal(1, #ids)
		local path = meta[ids[1]].path
		assert.truthy(path:find("Animals", 1, true))
		assert.truthy(path:find("Birds", 1, true))
	end)

	it("extracts a leaf object with name and captures synonyms in meta", function()
		local leaf = { name = "Cat", synonyms = { "Kitty", "Kitten" } }
		local vals, meta, ids = Util.extractAllKeywords({ leaf })
		assert.are.equal(1, #ids)
		local id = ids[1]
		assert.are.equal("Cat", vals[id])
		assert.are.same({ "Kitty", "Kitten" }, meta[id].synonyms)
	end)

	it("extracts a leaf object that has only a name (no synonyms)", function()
		local vals, meta, ids = Util.extractAllKeywords({ { name = "Dog" } })
		assert.are.equal(1, #ids)
		local id = ids[1]
		assert.are.equal("Dog", vals[id])
		assert.are.same({}, meta[id].synonyms)
	end)

	it("deduplicates the same keyword name under the same path", function()
		-- Both array entries resolve to the same (name="Rose", path="") pair.
		local _, _, ids = Util.extractAllKeywords({ "Rose", "Rose" })
		assert.are.equal(1, #ids)
	end)

	it("does NOT deduplicate the same keyword name under different paths", function()
		-- "Rose" under "Flowers" and "Plants" are distinct dedupe keys.
		local _, _, ids = Util.extractAllKeywords({
			Flowers = { "Rose" },
			Plants = { "Rose" },
		})
		assert.are.equal(2, #ids)
	end)

	it("skips empty string keywords", function()
		local _, _, ids = Util.extractAllKeywords({ "", "  ", "Valid" })
		assert.are.equal(1, #ids)
	end)

	it("skips leaf objects with an empty or whitespace-only name", function()
		local _, _, ids = Util.extractAllKeywords({ { name = "  " } })
		assert.are.equal(0, #ids)
	end)

	it("trims whitespace from keyword names", function()
		local vals, _, ids = Util.extractAllKeywords({ "  Trimmed  " })
		assert.are.equal(1, #ids)
		assert.are.equal("Trimmed", vals[ids[1]])
	end)

	it("returns an empty synonyms list in meta for a plain string leaf", function()
		local _, meta, ids = Util.extractAllKeywords({ "PlainWord" })
		assert.are.equal(1, #ids)
		assert.are.same({}, meta[ids[1]].synonyms)
	end)

	it("each meta entry contains synonyms, aliases, and synonymAliases fields", function()
		local _, meta, ids = Util.extractAllKeywords({ "Item" })
		local m = meta[ids[1]]
		assert.is_not_nil(m.synonyms)
		assert.is_not_nil(m.aliases)
		assert.is_not_nil(m.synonymAliases)
	end)

	it("sets path to empty string for a top-level flat keyword", function()
		local _, meta, ids = Util.extractAllKeywords({ "Solo" })
		assert.are.equal(1, #ids)
		assert.are.equal("", meta[ids[1]].path)
	end)

	it("handles multiple categories at the same level independently", function()
		local vals, _, ids = Util.extractAllKeywords({
			Fruit = { "Apple" },
			Vegetable = { "Carrot" },
		})
		assert.are.equal(2, #ids)
		assert.are.same({ "Apple", "Carrot" }, sortedNames(vals, ids))
	end)

	it("synonym is not duplicated in the synonyms list itself", function()
		-- The name and a synonym clash; the implementation strips the duplicate.
		local leaf = { name = "Cat", synonyms = { "Cat", "Kitty" } }
		local _, meta, ids = Util.extractAllKeywords({ leaf })
		assert.are.equal(1, #ids)
		-- "Cat" as a synonym would collide with the name and should be removed.
		local syns = meta[ids[1]].synonyms
		for _, s in ipairs(syns) do
			assert.are_not.equal("Cat", s)
		end
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.buildHierarchyFromPaths
-- ---------------------------------------------------------------------------

describe("Util.buildHierarchyFromPaths", function()
	it("returns an empty table for an empty input list", function()
		assert.are.same({}, Util.buildHierarchyFromPaths({}))
	end)

	it("inserts a single flat keyword (no '>') into the root as an array entry", function()
		local result = Util.buildHierarchyFromPaths({ { path = "Rose" } })
		assert.are.equal(1, #result)
		assert.are.equal("Rose", result[1])
	end)

	it("builds a two-level nested hierarchy from 'Animals > Dog'", function()
		local result = Util.buildHierarchyFromPaths({ { path = "Animals > Dog" } })
		assert.is_not_nil(result.Animals)
		assert.are.equal(1, #result.Animals)
		assert.are.equal("Dog", result.Animals[1])
	end)

	it("builds a three-level nested hierarchy from 'Animals > Birds > Owl'", function()
		local result = Util.buildHierarchyFromPaths({ { path = "Animals > Birds > Owl" } })
		assert.is_not_nil(result.Animals)
		assert.is_not_nil(result.Animals.Birds)
		local found = false
		for _, v in ipairs(result.Animals.Birds) do
			if v == "Owl" then
				found = true
			end
		end
		assert.is_true(found)
	end)

	it("stores a plain string leaf when no synonyms or aliases are present", function()
		local result = Util.buildHierarchyFromPaths({ { path = "Dog" } })
		assert.are.equal("Dog", result[1])
		assert.are.equal("string", type(result[1]))
	end)

	it("stores a name-object when synonyms are provided", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Cat", synonyms = { "Kitty", "Feline" } },
		})
		assert.are.equal(1, #result)
		local leaf = result[1]
		assert.are.equal("table", type(leaf))
		assert.are.equal("Cat", leaf.name)
		assert.are.same({ "Kitty", "Feline" }, leaf.synonyms)
	end)

	it("stores a name-object when only aliases are provided", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Wolf", aliases = { "Grey Wolf" } },
		})
		assert.are.equal(1, #result)
		local leaf = result[1]
		assert.are.equal("table", type(leaf))
		assert.are.equal("Wolf", leaf.name)
		assert.are.same({ "Grey Wolf" }, leaf.aliases)
	end)

	it("stores a name-object when only synonymAliases are provided", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Fox", synonymAliases = { "Red Fox" } },
		})
		assert.are.equal(1, #result)
		local leaf = result[1]
		assert.are.equal("table", type(leaf))
		assert.are.equal("Fox", leaf.name)
		assert.are.same({ "Red Fox" }, leaf.synonym_aliases)
	end)

	it("shares the same parent table for multiple paths under the same parent", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Fruit > Apple" },
			{ path = "Fruit > Banana" },
		})
		assert.is_not_nil(result.Fruit)
		assert.are.equal(2, #result.Fruit)
		local leaves = { result.Fruit[1], result.Fruit[2] }
		table.sort(leaves)
		assert.are.same({ "Apple", "Banana" }, leaves)
	end)

	it("creates distinct root keys for independent top-level paths", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Plants > Rose" },
			{ path = "Animals > Wolf" },
		})
		assert.is_not_nil(result.Plants)
		assert.is_not_nil(result.Animals)
		assert.are.equal("Rose", result.Plants[1])
		assert.are.equal("Wolf", result.Animals[1])
	end)

	it("trims whitespace from all path segments", function()
		local result = Util.buildHierarchyFromPaths({ { path = "  Food  >  Fruit  >  Apple  " } })
		assert.is_not_nil(result.Food)
		assert.is_not_nil(result.Food.Fruit)
		assert.are.equal("Apple", result.Food.Fruit[1])
	end)

	it("skips items with a path that contains only whitespace leaf segments", function()
		-- After trimming each part yields "", so the leaf is skipped.
		local result = Util.buildHierarchyFromPaths({ { path = "   >   " } })
		-- The root should be empty (the empty category segment is skipped as well).
		assert.are.equal(0, Util.table_size(result))
	end)

	it("sets synonym_aliases on a leaf node when synonymAliases are provided", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "Tags > Vintage", synonymAliases = { "retro", "old-school" } },
		})
		local leaf = result.Tags[1]
		assert.are.equal("table", type(leaf))
		assert.are.equal("Vintage", leaf.name)
		assert.are.same({ "retro", "old-school" }, leaf.synonym_aliases)
	end)

	it("accumulates multiple paths under a shared three-level ancestor", function()
		local result = Util.buildHierarchyFromPaths({
			{ path = "World > Europe > France" },
			{ path = "World > Europe > Germany" },
		})
		assert.is_not_nil(result.World)
		assert.is_not_nil(result.World.Europe)
		assert.are.equal(2, #result.World.Europe)
		local leaves = { result.World.Europe[1], result.World.Europe[2] }
		table.sort(leaves)
		assert.are.same({ "France", "Germany" }, leaves)
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.keywordTableToHierarchyStringList
-- ---------------------------------------------------------------------------

describe("Util.keywordTableToHierarchyStringList", function()
	it("returns an empty string for an empty table", function()
		assert.are.equal("", Util.keywordTableToHierarchyStringList({}))
	end)

	it("produces 'Category>Leaf' for a single one-level nested entry", function()
		local result = Util.keywordTableToHierarchyStringList({ Category = { "Leaf" } })
		assert.truthy(result:find("Category>Leaf", 1, true))
	end)

	it("produces 'A>B>C' for a two-level nested entry", function()
		local result = Util.keywordTableToHierarchyStringList({ A = { B = { "C" } } })
		assert.truthy(result:find("A>B>C", 1, true))
	end)

	it("produces a three-level path for a three-level nesting", function()
		local result = Util.keywordTableToHierarchyStringList({
			World = { Europe = { Germany = { "Berlin" } } },
		})
		assert.truthy(result:find("World>Europe>Germany>Berlin", 1, true))
	end)

	it("separates multiple paths with semicolons", function()
		local result = Util.keywordTableToHierarchyStringList({ Animals = { "Cat", "Dog" } })
		local parts = splitSemicolon(result)
		local hasCat, hasDog = false, false
		for _, p in ipairs(parts) do
			if p:find("Cat", 1, true) then
				hasCat = true
			end
			if p:find("Dog", 1, true) then
				hasDog = true
			end
		end
		assert.is_true(hasCat)
		assert.is_true(hasDog)
	end)

	it("produces exactly two entries for two leaves under one parent", function()
		local result = Util.keywordTableToHierarchyStringList({ Nature = { "Tree", "River" } })
		local parts = splitSemicolon(result)
		assert.are.equal(2, #parts)
	end)

	it("handles multiple independent top-level categories", function()
		local result = Util.keywordTableToHierarchyStringList({
			Fruit = { "Apple" },
			Vegetable = { "Carrot" },
		})
		local parts = splitSemicolon(result)
		assert.are.equal(2, #parts)
		local paths = {}
		for _, p in ipairs(parts) do
			paths[p] = true
		end
		assert.is_true(paths["Fruit>Apple"])
		assert.is_true(paths["Vegetable>Carrot"])
	end)

	it("treats a string value with a string key as parent>key>value", function()
		-- When v is a string and k is a string, the code emits path>k>v.
		-- With path={} (top level), concat gives "" so result is ">Color>Blue".
		local result = Util.keywordTableToHierarchyStringList({ Color = "Blue" })
		assert.truthy(result:find("Color", 1, true))
		assert.truthy(result:find("Blue", 1, true))
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.keywordTableToStringList
-- ---------------------------------------------------------------------------

describe("Util.keywordTableToStringList", function()
	it("returns an empty string for an empty table", function()
		assert.are.equal("", Util.keywordTableToStringList({}))
	end)

	it("extracts leaf strings from a one-level nested table", function()
		local result = Util.keywordTableToStringList({ Animals = { "Cat", "Dog" } })
		local parts = splitSemicolon(result)
		assert.are.equal(2, #parts)
		local have = {}
		for _, p in ipairs(parts) do
			have[p] = true
		end
		assert.is_true(have["Cat"])
		assert.is_true(have["Dog"])
	end)

	it("extracts leaf strings from a deeply nested table", function()
		local result = Util.keywordTableToStringList({ A = { B = { "Deep" } } })
		assert.truthy(result:find("Deep", 1, true))
	end)

	it("collects leaves from multiple independent top-level categories", function()
		local result = Util.keywordTableToStringList({
			Fruit = { "Mango" },
			Veg = { "Pea" },
		})
		local parts = splitSemicolon(result)
		assert.are.equal(2, #parts)
		local have = {}
		for _, p in ipairs(parts) do
			have[p] = true
		end
		assert.is_true(have["Mango"])
		assert.is_true(have["Pea"])
	end)

	it("does not include intermediate category keys in the output", function()
		local result = Util.keywordTableToStringList({ Animals = { "Cat" } })
		-- "Animals" is a category, not a leaf — it must not appear in the output.
		assert.is_nil(result:find("Animals", 1, true))
	end)

	it("result contains no hierarchy separator '>'", function()
		local result = Util.keywordTableToStringList({ X = { "Foo", "Bar" } })
		assert.is_nil(result:find(">", 1, true))
	end)

	it("handles a flat top-level array of strings", function()
		local result = Util.keywordTableToStringList({ "Apple", "Banana", "Cherry" })
		local parts = splitSemicolon(result)
		assert.are.equal(3, #parts)
		local have = {}
		for _, p in ipairs(parts) do
			have[p] = true
		end
		assert.is_true(have["Apple"])
		assert.is_true(have["Banana"])
		assert.is_true(have["Cherry"])
	end)

	it("returns a plain string with no trailing semicolon for a single leaf", function()
		local result = Util.keywordTableToStringList({ "OnlyOne" })
		assert.is_nil(result:match(";$"))
		assert.are.equal("OnlyOne", result)
	end)
end)

-- ---------------------------------------------------------------------------
-- Util.formatTimestampSafe — string-transformation logic.
--
-- The function body is:
--   timestamp = timestamp or LrDate.currentTime()
--   local w3c = LrDate.timeToW3CDate(timestamp)
--   return w3c:gsub("T", "_"):gsub(":", "-"):sub(1, 19)
--
-- In the headless test environment LrDate is a stub whose methods return a
-- noop function — not a string — so calling Util.formatTimestampSafe()
-- directly would crash on :gsub.  We therefore test the string-transformation
-- rule directly by mirroring the exact three operations from the source.
-- ---------------------------------------------------------------------------

describe("Util.formatTimestampSafe (string transform rule)", function()
	-- Mirror of the production transform.
	local function applyTransform(w3c)
		return w3c:gsub("T", "_"):gsub(":", "-"):sub(1, 19)
	end

	it("replaces the 'T' separator with '_'", function()
		local result = applyTransform("2024-05-15T10:30:00Z")
		assert.is_nil(result:find("T", 1, true))
		assert.truthy(result:find("_", 1, true))
	end)

	it("replaces ':' colons in the time portion with '-'", function()
		local result = applyTransform("2024-05-15T10:30:00Z")
		assert.is_nil(result:find(":", 1, true))
	end)

	it("truncates to exactly 19 characters", function()
		local result = applyTransform("2024-05-15T10:30:00Z")
		assert.are.equal(19, #result)
	end)

	it("produces the expected filesystem-safe string for a known input", function()
		local result = applyTransform("2024-05-15T10:30:00Z")
		assert.are.equal("2024-05-15_10-30-00", result)
	end)

	it("handles midnight correctly", function()
		local result = applyTransform("2000-01-01T00:00:00Z")
		assert.are.equal("2000-01-01_00-00-00", result)
	end)

	it("handles end-of-day time correctly", function()
		local result = applyTransform("1999-12-31T23:59:59Z")
		assert.are.equal("1999-12-31_23-59-59", result)
	end)
end)
