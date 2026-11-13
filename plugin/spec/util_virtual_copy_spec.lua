-- Unit tests for Util.isVirtualCopyOf, the check AI Edit runs after
-- `catalog:createVirtualCopies()` to make sure the copy it is about to edit
-- came from the photo it meant to copy.
--
-- Plain tables stand in for LrPhoto objects: the helper only compares identity.
--
-- Run from the repo root with:  busted

local Util = require("Util")

describe("Util.isVirtualCopyOf", function()
	local master = { name = "master" }
	local siblingCopy = { name = "virtual copy of master" }
	local otherPhoto = { name = "unrelated photo" }

	it("accepts a copy of a master photo", function()
		assert.is_true(Util.isVirtualCopyOf(master, master, false, nil))
		-- isVirtualCopy may come back nil rather than false for a master.
		assert.is_true(Util.isVirtualCopyOf(master, master, nil, nil))
	end)

	it("accepts a copy of a virtual copy, which belongs to the same master", function()
		assert.is_true(Util.isVirtualCopyOf(master, siblingCopy, true, master))
	end)

	it("rejects a copy of a virtual copy whose master is a different photo", function()
		assert.is_false(Util.isVirtualCopyOf(otherPhoto, siblingCopy, true, master))
	end)

	it("rejects a copy that belongs to a genuinely different photo", function()
		assert.is_false(Util.isVirtualCopyOf(otherPhoto, master, false, nil))
	end)

	it("ignores sourceMaster for a master source", function()
		-- Only a virtual-copy source may be matched through its master; a stray
		-- sourceMaster must not let a copy of another photo through.
		assert.is_false(Util.isVirtualCopyOf(otherPhoto, master, false, otherPhoto))
		assert.is_false(Util.isVirtualCopyOf(otherPhoto, master, nil, otherPhoto))
	end)

	it("rejects when the master cannot be read", function()
		assert.is_false(Util.isVirtualCopyOf(nil, master, false, nil))
		assert.is_false(Util.isVirtualCopyOf(master, siblingCopy, true, nil))
	end)
end)
