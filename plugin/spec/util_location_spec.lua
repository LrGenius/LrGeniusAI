-- Unit tests for Util.getPhotoLocation.
--
-- The backend used to read the place a photo was taken out of the image bytes
-- it was handed, which found nothing for a raw original or a full-size JPEG:
-- normalising those re-encodes them, and the JPEG that comes out has neither
-- EXIF nor IPTC (issue #321). Reading the catalog instead is what makes the
-- location independent of what survives a conversion.

local Util = require("Util")

--- A photo double with the given formatted metadata and raw GPS.
local function photoWith(formatted, gps)
	return {
		getFormattedMetadata = function(_, key)
			return formatted[key]
		end,
		getRawMetadata = function(_, key)
			if key == "gps" then
				return gps
			end
			return nil
		end,
	}
end

describe("Util.getPhotoLocation", function()
	it("maps Lightroom's location fields onto the request fields", function()
		local photo = photoWith({
			location = "Praia das Catedrais",
			city = "Ribadeo",
			stateProvince = "Galicia",
			country = "Spain",
			isoCountryCode = "ES",
		}, { latitude = 43.537, longitude = -7.0409 })

		assert.are.same({
			location_sublocation = "Praia das Catedrais",
			location_city = "Ribadeo",
			location_state = "Galicia",
			location_country = "Spain",
			location_country_code = "ES",
			gps_latitude = 43.537,
			gps_longitude = -7.0409,
		}, Util.getPhotoLocation(photo))
	end)

	it("sends coordinates alone when the address was never confirmed", function()
		-- The common case behind issue #321: Lightroom's address lookup only
		-- ever suggested a city, so nothing but the GPS fix exists. The backend
		-- turns these into a place name itself.
		local photo = photoWith({}, { latitude = 48.45964, longitude = 12.089297 })
		assert.are.same({ gps_latitude = 48.45964, gps_longitude = 12.089297 }, Util.getPhotoLocation(photo))
	end)

	it("drops empty and blank fields rather than sending them", function()
		-- An empty city still reads as "a place is known" on the far side and
		-- would stop the coordinates from being looked up.
		local photo = photoWith({ city = "  ", country = "Spain" }, nil)
		assert.are.same({ location_country = "Spain" }, Util.getPhotoLocation(photo))
	end)

	it("ignores half a coordinate pair", function()
		local photo = photoWith({}, { latitude = 43.537 })
		assert.are.same({}, Util.getPhotoLocation(photo))
	end)

	it("returns an empty table when the catalog knows nothing", function()
		assert.are.same({}, Util.getPhotoLocation(photoWith({}, nil)))
		assert.are.same({}, Util.getPhotoLocation(nil))
	end)

	it("survives a photo whose metadata cannot be read", function()
		local unreadable = {
			getFormattedMetadata = function()
				error("photo is offline")
			end,
			getRawMetadata = function()
				error("photo is offline")
			end,
		}
		assert.are.same({}, Util.getPhotoLocation(unreadable))
	end)
end)
