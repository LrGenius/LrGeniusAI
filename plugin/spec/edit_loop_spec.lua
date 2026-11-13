-- Tests for AI Edit's retoucher loop, Lightroom side: how each round's table is
-- put together from the photo's starting point, how masks are merged by sync
-- id, and what Lightroom's mask states tell the model. Applying, rendering and
-- the backend need Lightroom; these decisions do not.

require("EditLoop")

local function sky(amount)
	return {
		CorrectionName = "LrGenius · Sky",
		CorrectionSyncID = "SKY",
		LocalExposure2012 = amount,
		CorrectionMasks = { { What = "Mask/Image", MaskSubType = 2 } },
	}
end

local function subject()
	return {
		CorrectionName = "LrGenius · Subject",
		CorrectionSyncID = "SUBJECT",
		CorrectionMasks = { { What = "Mask/Image", MaskSubType = 1 } },
	}
end

local function own()
	return {
		CorrectionName = "Mask 1",
		CorrectionSyncID = "MINE",
		CorrectionMasks = { { What = "Mask/Image", MaskSubType = 1, MaskDigest = "ABC" } },
	}
end

describe("EditLoop.composeApply", function()
	it("applies a first round as it came", function()
		local state = {}
		local out = EditLoop.composeApply({ Exposure2012 = 0.2 }, { Exposure2012 = 0.6, Contrast2012 = 10 }, state)

		assert.are.equal(0.6, out.Exposure2012)
		assert.are.equal(10, out.Contrast2012)
		assert.is_nil(out.MaskGroupBasedCorrections)
		assert.is_true(state.touched.Contrast2012)
	end)

	it("puts a slider a later round leaves out back to its starting value", function()
		local baseline = { Exposure2012 = 0.2, Contrast2012 = 5, Vibrance = 0 }
		local state = {}
		EditLoop.composeApply(baseline, { Exposure2012 = 0.6, Vibrance = 20 }, state)
		local second = EditLoop.composeApply(baseline, { Exposure2012 = 0.5 }, state)

		assert.are.equal(0.5, second.Exposure2012)
		assert.are.equal(0, second.Vibrance)
		assert.is_nil(second.Contrast2012, "a slider no round touched is not written")
	end)

	it("keeps the photo's own masks and replaces its own by sync id", function()
		local baseline = { MaskGroupBasedCorrections = { own() } }
		local state = {}
		local first = EditLoop.composeApply(baseline, { MaskGroupBasedCorrections = { sky(-0.1), subject() } }, state)
		assert.are.equal(3, #first.MaskGroupBasedCorrections)
		assert.are.equal("MINE", first.MaskGroupBasedCorrections[1].CorrectionSyncID)

		local second = EditLoop.composeApply(baseline, { MaskGroupBasedCorrections = { sky(-0.2) } }, state)
		local ids = {}
		for _, c in ipairs(second.MaskGroupBasedCorrections) do
			table.insert(ids, c.CorrectionSyncID)
		end
		assert.are.same({ "MINE", "SKY" }, ids, "round 2 dropped the subject, and the sky is round 2's")
		assert.are.equal(-0.2, second.MaskGroupBasedCorrections[2].LocalExposure2012)
	end)

	it("replaces a mask an earlier AI Edit run left on the photo", function()
		local baseline = { MaskGroupBasedCorrections = { own(), sky(-0.9) } }
		local out = EditLoop.composeApply(baseline, { MaskGroupBasedCorrections = { sky(-0.3) } }, {})

		assert.are.equal(2, #out.MaskGroupBasedCorrections)
		assert.are.equal(-0.3, out.MaskGroupBasedCorrections[2].LocalExposure2012)
	end)

	it("leaves the masks alone when no round has any", function()
		local baseline = { MaskGroupBasedCorrections = { own() } }
		local out = EditLoop.composeApply(baseline, { Exposure2012 = 0.1 }, {})
		assert.is_nil(out.MaskGroupBasedCorrections)
	end)
end)

describe("EditLoop.composeRevert", function()
	it("takes back every slider and mask the loop wrote", function()
		local baseline = { Exposure2012 = 0.2, Vibrance = 0, MaskGroupBasedCorrections = { own() } }
		local state = {}
		EditLoop.composeApply(
			baseline,
			{ Exposure2012 = 0.6, Vibrance = 20, MaskGroupBasedCorrections = { sky(-0.2) } },
			state
		)
		local out = EditLoop.composeRevert(baseline, state)

		assert.are.equal(0.2, out.Exposure2012)
		assert.are.equal(0, out.Vibrance)
		assert.are.equal(1, #out.MaskGroupBasedCorrections)
		assert.are.equal("MINE", out.MaskGroupBasedCorrections[1].CorrectionSyncID)
	end)

	it("clears the masks when the photo had none of its own", function()
		local state = {}
		EditLoop.composeApply({}, { MaskGroupBasedCorrections = { sky(-0.2) } }, state)
		local out = EditLoop.composeRevert({}, state)
		assert.are.same({}, out.MaskGroupBasedCorrections)
	end)
end)

describe("EditLoop.maskState", function()
	it("reads computed, nothing found and pending the way the experiments did", function()
		assert.are.equal("computed", EditLoop.maskState(own()))
		assert.are.equal("pending", EditLoop.maskState(sky(0)))
		local empty = sky(0)
		empty.CorrectionMasks[1].ErrorReason = 1
		assert.are.equal("nothing_found", EditLoop.maskState(empty))
		assert.are.equal("missing", EditLoop.maskState(nil))
	end)
end)

describe("EditLoop.roundMaskStates and maskNotes", function()
	it("tells the model about a mask that found nothing, and nothing about one that worked", function()
		local onPhoto = sky(0)
		onPhoto.CorrectionMasks[1].ErrorReason = 1
		local computedSubject = subject()
		computedSubject.CorrectionMasks[1].MaskDigest = "X"
		local settings = { MaskGroupBasedCorrections = { own(), onPhoto, computedSubject } }
		local states = EditLoop.roundMaskStates(settings, { MaskGroupBasedCorrections = { sky(0), subject() } })

		assert.are.equal(2, #states)
		local notes = EditLoop.maskNotes(states)
		assert.are.equal(1, #notes)
		assert.is_truthy(notes[1]:find("Sky mask found nothing", 1, true))
	end)
end)

describe("EditLoop.supportsMasks", function()
	it("needs Lightroom 15.3 or later", function()
		assert.is_false(EditLoop.supportsMasks({ major = 15, minor = 2 }))
		assert.is_true(EditLoop.supportsMasks({ major = 15, minor = 3 }))
		assert.is_true(EditLoop.supportsMasks({ major = 16, minor = 0 }))
		assert.is_false(EditLoop.supportsMasks(nil))
	end)
end)
