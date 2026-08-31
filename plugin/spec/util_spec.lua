-- Unit tests for the pure-logic helpers in Util.lua.
--
-- These run headless (plain Lua + busted, no Lightroom). They cover the string
-- and table utilities that underpin keyword handling, photo IDs and search —
-- exactly the kind of logic where a silent off-by-one slips into a release.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.table_contains", function()
	it("finds a value that is present", function()
		assert.is_true(Util.table_contains({ "a", "b", "c" }, "b"))
	end)

	it("returns false for a value that is absent", function()
		assert.is_false(Util.table_contains({ "a", "b" }, "z"))
	end)

	it("works over non-array (hash) tables", function()
		assert.is_true(Util.table_contains({ x = 1, y = 2 }, 2))
	end)

	it("returns false for an empty table", function()
		assert.is_false(Util.table_contains({}, "a"))
	end)
end)

describe("Util.trim", function()
	it("strips leading and trailing whitespace", function()
		assert.are.equal("hello", Util.trim("  hello  "))
	end)

	it("leaves internal whitespace intact", function()
		assert.are.equal("a b c", Util.trim("  a b c "))
	end)

	it("returns an empty string for all-whitespace input", function()
		assert.are.equal("", Util.trim("   \t  "))
	end)
end)

describe("Util.nilOrEmpty", function()
	it("is true for nil", function()
		assert.is_true(Util.nilOrEmpty(nil))
	end)

	it("is true for an empty or whitespace-only string", function()
		assert.is_true(Util.nilOrEmpty(""))
		assert.is_true(Util.nilOrEmpty("   "))
	end)

	it("is false for a non-empty string", function()
		assert.is_false(Util.nilOrEmpty("x"))
	end)

	it("is false for a non-nil, non-string value", function()
		assert.is_false(Util.nilOrEmpty(0))
		assert.is_false(Util.nilOrEmpty({}))
	end)
end)

describe("Util.string_split", function()
	it("splits on the delimiter and trims each part", function()
		assert.are.same({ "a", "b", "c" }, Util.string_split("a, b ,c", ","))
	end)

	it("skips empty segments", function()
		-- pattern matches runs of non-delimiter chars, so empties collapse
		assert.are.same({ "a", "b" }, Util.string_split("a,,b", ","))
	end)
end)

describe("Util.split", function()
	it("keeps empty segments and does plain (non-pattern) matching", function()
		assert.are.same({ "a", "", "b" }, Util.split("a..b", "."))
	end)

	it("returns the whole string when the delimiter is absent", function()
		assert.are.same({ "abc" }, Util.split("abc", "/"))
	end)

	it("splits hierarchy strings", function()
		assert.are.same({ "Animal", "Bird", "Owl" }, Util.split("Animal>Bird>Owl", ">"))
	end)
end)

describe("Util.table_size", function()
	it("counts array entries", function()
		assert.are.equal(3, Util.table_size({ 1, 2, 3 }))
	end)

	it("counts hash entries", function()
		assert.are.equal(2, Util.table_size({ a = 1, b = 2 }))
	end)

	it("is zero for an empty table", function()
		assert.are.equal(0, Util.table_size({}))
	end)
end)

describe("Util.get_keys", function()
	it("returns all keys of a hash table", function()
		local keys = Util.get_keys({ a = 1, b = 2 })
		table.sort(keys)
		assert.are.same({ "a", "b" }, keys)
	end)
end)

describe("Util.deepcopy", function()
	it("produces an independent copy of nested tables", function()
		local original = { a = 1, nested = { b = 2 } }
		local copy = Util.deepcopy(original)
		copy.nested.b = 99
		assert.are.equal(2, original.nested.b)
		assert.are.equal(99, copy.nested.b)
	end)

	it("handles cyclic references without recursing forever", function()
		local t = {}
		t.self = t
		local copy = Util.deepcopy(t)
		assert.are.equal(copy, copy.self)
		assert.are_not.equal(t, copy)
	end)

	it("returns nil for nil", function()
		assert.is_nil(Util.deepcopy(nil))
	end)
end)

describe("Util.keywordToHierarchyString", function()
	-- Build a fake LrKeyword chain: Owl's parent is Bird, Bird's parent is Animal.
	local function fakeKeyword(name, parent)
		return {
			getName = function()
				return name
			end,
			getParent = function()
				return parent
			end,
		}
	end

	it("joins the keyword ancestry top-down with '>'", function()
		local animal = fakeKeyword("Animal", nil)
		local bird = fakeKeyword("Bird", animal)
		local owl = fakeKeyword("Owl", bird)
		assert.are.equal("Animal>Bird>Owl", Util.keywordToHierarchyString(owl))
	end)

	it("returns just the name for a top-level keyword", function()
		assert.are.equal("Animal", Util.keywordToHierarchyString(fakeKeyword("Animal", nil)))
	end)
end)

describe("Util.promptMenuItems", function()
	it("puts the default prompt first and sorts the rest alphabetically", function()
		local items = Util.promptMenuItems({
			["Zoo Visit"] = "z",
			["Default"] = "d",
			["Family & Everyday Photos"] = "f",
		}, "Default")
		assert.are.same({
			{ title = "Default", value = "Default" },
			{ title = "Family & Everyday Photos", value = "Family & Everyday Photos" },
			{ title = "Zoo Visit", value = "Zoo Visit" },
		}, items)
	end)

	it("orders case-insensitively and stays deterministic for equal names", function()
		local items = Util.promptMenuItems({ ["beach"] = "1", ["Album"] = "2", ["Beach"] = "3" }, nil)
		assert.are.same({ "Album", "Beach", "beach" }, {
			items[1].title,
			items[2].title,
			items[3].title,
		})
	end)

	it("returns an empty list rather than failing on nil", function()
		assert.are.same({}, Util.promptMenuItems(nil, "Default"))
	end)

	it("keeps every prompt when the pinned name is not there", function()
		local items = Util.promptMenuItems({ ["One"] = "1", ["Two"] = "2" }, "Default")
		assert.are.equal(2, #items)
	end)
end)

describe("Util.seedBuiltinPrompts", function()
	local builtins = {
		{ name = "Default", instruction = "documentary" },
		{ name = "Family & Everyday Photos", instruction = "album voice" },
	}

	it("seeds both prompts on a fresh install", function()
		local prompts, seeded, changed = Util.seedBuiltinPrompts(nil, nil, builtins)
		assert.is_true(changed)
		assert.are.equal("documentary", prompts["Default"])
		assert.are.equal("album voice", prompts["Family & Everyday Photos"])
		assert.is_true(seeded["Default"])
	end)

	it("adds a newly shipped prompt to an existing install", function()
		-- What an upgrade looks like: the old default is there, edited, and the
		-- new preset has never been offered.
		local prompts, _, changed = Util.seedBuiltinPrompts(
			{ Default = "my own wording" },
			{ Default = true },
			builtins
		)
		assert.is_true(changed)
		assert.are.equal("my own wording", prompts["Default"])
		assert.are.equal("album voice", prompts["Family & Everyday Photos"])
	end)

	it("does not resurrect a prompt the user deleted", function()
		local prompts, _, changed = Util.seedBuiltinPrompts(
			{ Default = "documentary" },
			{ ["Default"] = true, ["Family & Everyday Photos"] = true },
			builtins
		)
		assert.is_nil(prompts["Family & Everyday Photos"])
		assert.is_false(changed)
	end)

	it("leaves a user's own prompt of the same name alone", function()
		local prompts = Util.seedBuiltinPrompts({ ["Family & Everyday Photos"] = "mine" }, {}, builtins)
		assert.are.equal("mine", prompts["Family & Everyday Photos"])
	end)

	it("reports no change once everything has been offered", function()
		local prompts, seeded = Util.seedBuiltinPrompts(nil, nil, builtins)
		local _, _, changed = Util.seedBuiltinPrompts(prompts, seeded, builtins)
		assert.is_false(changed)
	end)
end)
