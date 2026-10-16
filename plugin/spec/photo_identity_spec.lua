-- Regressions for #397: real identity and scan code, with SDK/HTTP boundaries
-- replaced by doubles. Any photo write advances lastEditTime and fails.
local Util = require("Util")
require("APISearchIndex")
local json = require("JSON")

describe("photo identity without photo metadata writes", function()
	local saved, catalog, photos, requests, attributes, writes, scope

	local function newCatalog(properties)
		local cat = { properties = properties or {}, writes = 0 }
		function cat:getPropertyForPlugin(_, key)
			return self.properties[key]
		end
		function cat:setPropertyForPlugin(_, key, value)
			assert.is_true(self.hasPrivateWriteAccess or self.hasWriteAccess)
			self.writes = self.writes + 1
			self.properties[key] = value
		end
		function cat:withPrivateWriteAccessDo(fn)
			assert.is_falsy(self.hasPrivateWriteAccess or self.hasWriteAccess, "nested write access")
			self.hasPrivateWriteAccess = true
			fn()
			self.hasPrivateWriteAccess = false
		end
		function cat:getAllPhotos()
			return photos
		end
		function cat:findPhotoByUuid(uuid)
			for _, photo in ipairs(photos) do
				if photo.raw.uuid == uuid then
					return photo
				end
			end
		end
		return cat
	end

	local function newPhoto(uuid, properties)
		local photo = {
			catalog = catalog,
			raw = { uuid = uuid, path = "/photos/" .. uuid, dateTime = 123, width = 100, lastEditTime = 42 },
			formatted = { fileName = uuid .. ".jpg" },
			properties = properties or {},
		}
		function photo:getRawMetadata(key)
			return self.raw[key]
		end
		function photo:getFormattedMetadata(key)
			return self.formatted[key]
		end
		function photo:getPropertyForPlugin(_, key)
			return self.properties[key]
		end
		function photo:setPropertyForPlugin()
			writes = writes + 1
			self.raw.lastEditTime = 99
			error("photo metadata must not be written")
		end
		photo.setRawMetadata = photo.setPropertyForPlugin
		photos[#photos + 1] = photo
		return photo
	end

	local function legacy(algorithm)
		return {
			globalPhotoId = algorithm == "md5_partial" and "md5p:100:7:old" or "meta1:existing",
			globalPhotoIdAlgorithm = algorithm or "stable_meta_v1",
			globalPhotoIdFileSize = "100",
			globalPhotoIdFileModificationDate = "7",
		}
	end

	before_each(function()
		saved = {}
		for _, name in ipairs({
			"LrApplication",
			"LrFileUtils",
			"LrMD5",
			"LrDate",
			"LrTasks",
			"LrHttp",
			"LrProgressScope",
			"JSON",
			"prefs",
			"PhotoSelector",
			"ErrorHandler",
		}) do
			saved[name] = { value = _G[name] }
		end
		saved.ping = SearchIndexAPI.pingServer
		photos, requests, writes = {}, {}, 0
		attributes = { fileSize = 100, fileModificationDate = 7.5 }
		catalog = newCatalog({ catalogIdentifier = "cat_test", catalogDbMigrations = "claim_photos_v1" })
		_G.LrApplication = {
			activeCatalog = function()
				return catalog
			end,
		}
		_G.LrFileUtils = {
			exists = function()
				return attributes and "file"
			end,
			isReadable = function()
				return attributes ~= nil
			end,
			fileAttributes = function()
				return attributes
			end,
		}
		_G.LrMD5 = {
			digest = function(payload)
				return "digest:" .. payload
			end,
		}
		_G.LrDate = {
			currentTime = function()
				return 1000
			end,
		}
		_G.LrTasks = { pcall = saved.LrTasks.value.pcall, yield = function() end }
		_G.JSON = json
		_G.prefs = { backendServerUrl = "https://backend.example", useGlobalPhotoId = true }
		_G.LrHttp = {
			post = function(url, body)
				local request = json:decode(body)
				requests[#requests + 1] = { url = url, body = request }
				return json:encode({
					claimed = #(request.photo_ids or {}),
					errors = 0,
					photo_ids = request.photo_ids,
					results = {},
					disassociated = 0,
				}),
					{ status = 200 }
			end,
		}
		scope = {
			setCaption = function() end,
			setPortionComplete = function() end,
			isCanceled = function()
				return false
			end,
			done = function() end,
		}
		_G.LrProgressScope = function()
			return scope
		end
		_G.PhotoSelector = {
			getPhotosInScope = function()
				return photos
			end,
		}
		_G.ErrorHandler = {
			handleError = function(title, detail)
				error(title .. ": " .. tostring(detail))
			end,
		}
		SearchIndexAPI.pingServer = function()
			return true
		end
	end)

	after_each(function()
		SearchIndexAPI.pingServer = saved.ping
		saved.ping = nil
		for name, original in pairs(saved) do
			_G[name] = original.value
		end
		assert.are.equal(0, writes, "a scan attempted to write photo metadata")
		for _, photo in ipairs(photos) do
			assert.are.equal(42, photo.raw.lastEditTime)
		end
	end)

	it("persists a cold-cache ID in the catalog and reuses it without writes", function()
		local photo = newPhoto("one")
		local expected = Util.computeStableMetadataPhotoId(photo)
		assert.are.equal(expected, Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal(1, catalog.writes)
		assert.same({}, photo.properties)
		assert.are.equal(expected, Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal(1, catalog.writes)
	end)

	it("retains the ID across a rename, changed metadata, file rewrite and restored catalog", function()
		local photo = newPhoto("one")
		local id = assert(Util.getGlobalPhotoIdForPhoto(photo))
		catalog = newCatalog(catalog.properties)
		local reopened = newPhoto("one")
		reopened.formatted.fileName = "renamed.dng"
		reopened.raw.path, reopened.raw.dateTime = "/new/location.dng", 999
		attributes = { fileSize = 200, fileModificationDate = 100 }
		assert.are.equal(id, Util.getGlobalPhotoIdForPhoto(reopened))
		assert.are.equal(0, catalog.writes)
	end)

	it("isolates equal UUIDs in different catalogs and uses the photo's owning catalog", function()
		local first = newPhoto("one")
		local id = assert(Util.getGlobalPhotoIdForPhoto(first))
		catalog = newCatalog()
		local second = newPhoto("one")
		second.formatted.fileName = "different.jpg"
		assert.are_not.equal(id, Util.getGlobalPhotoIdForPhoto(second))
		assert.are.equal(id, Util.getGlobalPhotoIdForPhoto(first))
	end)

	for _, algorithm in ipairs({ "stable_meta_v1", "md5_partial" }) do
		it("migrates existing " .. algorithm .. " IDs without changing or clearing photo properties", function()
			local fields = legacy(algorithm)
			local photo = newPhoto("one", fields)
			assert.are.equal(fields.globalPhotoId, Util.getGlobalPhotoIdForPhoto(photo))
			assert.same(legacy(algorithm), photo.properties)
			assert.are.equal(fields.globalPhotoId, Util.getGlobalPhotoIdForPhoto(photo))
			assert.are.equal(1, catalog.writes)
		end)

		it("keeps offline " .. algorithm .. " photos associated with their existing ID", function()
			attributes = nil
			local fields = legacy(algorithm)
			local photo = newPhoto("offline", fields)
			assert.are.equal(fields.globalPhotoId, Util.getGlobalPhotoIdForPhoto(photo))
			assert.are.equal(fields.globalPhotoId, Util.getGlobalPhotoIdForPhoto(photo))
		end)
	end

	it("resolves uncached offline photos from catalog metadata", function()
		attributes = nil
		local photo = newPhoto("offline")
		assert.are.equal(Util.computeStableMetadataPhotoId(photo), Util.getGlobalPhotoIdForPhoto(photo))
	end)

	it("recomputes a changed legacy hash and gives the catalog cache precedence", function()
		local photo = newPhoto("one", legacy("md5_partial"))
		assert.are.equal("md5p:100:7:old", Util.getGlobalPhotoIdForPhoto(photo))
		attributes.fileSize = 200
		local id = assert(Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal(Util.computeStableMetadataPhotoId(photo), id)
		assert.are.equal(id, Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal("md5p:100:7:old", photo.properties.globalPhotoId)
	end)

	it("forceRecompute replaces only the catalog cache", function()
		local photo = newPhoto("one", legacy())
		assert.are.equal("meta1:existing", Util.getGlobalPhotoIdForPhoto(photo))
		local id = assert(Util.getGlobalPhotoIdForPhoto(photo, { forceRecompute = true }))
		assert.are_not.equal("meta1:existing", id)
		assert.are.equal(id, Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal("meta1:existing", photo.properties.globalPhotoId)
	end)

	it("uses the unchanged partial hash fallback when metadata is insufficient", function()
		local path = os.tmpname()
		local file = assert(io.open(path, "wb"))
		file:write(string.rep("x", 100))
		file:close()
		local photo = newPhoto("one")
		photo.raw.path, photo.raw.dateTime, photo.raw.width = path, nil, nil
		photo.formatted = {}
		local expected = Util.buildGlobalPhotoId(path, 4)
		local id, err = Util.getGlobalPhotoIdForPhoto(photo, { windowBytes = 4 })
		os.remove(path)
		assert.is_nil(err)
		assert.are.equal(expected, id)
		attributes = nil
		assert.are.equal(id, Util.getGlobalPhotoIdForPhoto(photo))
	end)

	it("returns an error for unavailable originals with no usable metadata", function()
		attributes = nil
		local photo = newPhoto("one")
		photo.raw.dateTime, photo.raw.width, photo.formatted = nil, nil, {}
		local id, err = Util.getGlobalPhotoIdForPhoto(photo)
		assert.is_nil(id)
		assert.is_truthy(err)
		assert.are.equal(0, catalog.writes)
	end)

	it("does not invent a replacement identity for a corrupt durable record", function()
		local photo = newPhoto("one")
		catalog.properties.photoIdentityV1_one = "broken JSON"
		local id, err = Util.getGlobalPhotoIdForPhoto(photo)
		assert.is_nil(id)
		assert.matches("Could not read photo identity", err, 1, true)
		assert.are.equal(0, catalog.writes)
	end)

	it("does not return an ID when persisting it fails", function()
		catalog.setPropertyForPlugin = function()
			error("catalog is read-only")
		end
		local id, err = Util.getGlobalPhotoIdForPhoto(newPhoto("one"))
		assert.is_nil(id)
		assert.matches("Check catalog write access", err, 1, true)
	end)

	it("reuses existing catalog write access", function()
		catalog.hasPrivateWriteAccess = true
		assert.is_truthy(Util.getGlobalPhotoIdForPhoto(newPhoto("one")))
	end)

	it("uses an identity persisted by a concurrent resolver while awaiting write access", function()
		local photo = newPhoto("one")
		local normalWrite = catalog.withPrivateWriteAccessDo
		catalog.withPrivateWriteAccessDo = function(self, fn)
			self.properties.photoIdentityV1_one = json:encode({ id = "meta1:concurrent", algorithm = "stable_meta_v1" })
			normalWrite(self, fn)
		end
		assert.are.equal("meta1:concurrent", Util.getGlobalPhotoIdForPhoto(photo))
		assert.are.equal(0, catalog.writes)
	end)

	it("claims the full catalog in batches with unchanged IDs and zero photo writes", function()
		local expected = {}
		for i = 1, 2501 do
			local photo = newPhoto(tostring(i))
			expected[i] = Util.computeStableMetadataPhotoId(photo)
		end
		local ok, err, result = SearchIndexAPI.claimPhotosForCatalog(scope)
		assert.is_true(ok)
		assert.is_nil(err)
		assert.are.equal(2501, result.claimed)
		assert.are.equal(2, #requests)
		assert.are.equal(2500, #requests[1].body.photo_ids)
		assert.same({ expected[2501] }, requests[2].body.photo_ids)
		assert.are.equal("cat_test", requests[1].body.catalog_id)
		assert.are.equal(expected[1], requests[1].body.photo_ids[1])
		assert.matches("/v1/photos/catalogs/claim", requests[1].url, 1, true)
	end)

	it("does not mark an incomplete claim as successful or send an incomplete cleanup inventory", function()
		newPhoto("one")
		catalog.setPropertyForPlugin = function()
			error("write failed")
		end
		local ok, err = SearchIndexAPI.claimPhotosForCatalog(scope)
		assert.is_false(ok)
		assert.matches("Could not resolve photo identity for claiming", err, 1, true)
		ok, err = SearchIndexAPI.syncCleanup()
		assert.is_false(ok)
		assert.matches("Could not resolve photo identity for cleanup", err, 1, true)
		assert.are.equal(0, #requests)
	end)

	it("honors cancellation before resolving or sending claims", function()
		newPhoto("one")
		scope.isCanceled = function()
			return true
		end
		local ok, err = SearchIndexAPI.claimPhotosForCatalog(scope)
		assert.is_false(ok)
		assert.are.equal("canceled", err)
		assert.are.equal(0, catalog.writes)
		assert.are.equal(0, #requests)
	end)

	it("finds cold-cache and migrated IDs in requested order, retaining duplicates", function()
		local first = newPhoto("one")
		local second = newPhoto("two", legacy())
		local id = Util.computeStableMetadataPhotoId(first)
		assert.same(
			{ second, first, second },
			SearchIndexAPI.findPhotosByPhotoIds({ "meta1:existing", id, "meta1:existing" })
		)
		assert.are.equal(first, SearchIndexAPI.findPhotoByPhotoId(id))
		assert.is_nil(SearchIndexAPI.findPhotoByPhotoId("missing"))
	end)

	it("matches results after a restart using catalog IDs even when old photo IDs differ", function()
		local photo = newPhoto("one", legacy())
		local id = assert(Util.getGlobalPhotoIdForPhoto(photo, { forceRecompute = true }))
		catalog = newCatalog(catalog.properties)
		local reopened = newPhoto("one", legacy())
		photos = { reopened }
		assert.are.equal(reopened, SearchIndexAPI.findPhotoByPhotoId(id))
		assert.same({}, SearchIndexAPI.findPhotosByPhotoIds({ "meta1:existing" }))
	end)

	it("keeps the Lightroom UUID mode free of identity cache writes", function()
		prefs.useGlobalPhotoId = false
		local photo = newPhoto("one")
		assert.are.equal("one", SearchIndexAPI.getPhotoIdForPhoto(photo))
		assert.are.equal(photo, SearchIndexAPI.findPhotoByPhotoId("one"))
		assert.is_true(SearchIndexAPI.claimPhotosForCatalog())
		assert.same({ "one" }, requests[1].body.photo_ids)
		assert.are.equal(0, catalog.writes)
	end)

	it("scoped search, missing-photo scans and cleanup use the same IDs without photo writes", function()
		local photo = newPhoto("one")
		local id = Util.computeStableMetadataPhotoId(photo)
		assert.is_truthy(SearchIndexAPI.searchIndex("forest", photos))
		assert.same({ id }, requests[1].body.photo_ids)
		local ok, missing = SearchIndexAPI.getMissingPhotosFromIndex({ enableEmbeddings = true }, scope)
		assert.is_true(ok)
		assert.same({ photo }, missing)
		assert.same({ id }, requests[2].body.photo_ids)
		assert.is_true(SearchIndexAPI.syncCleanup())
		assert.same({ id }, requests[3].body.photo_ids)
		assert.are.equal(1, catalog.writes)
	end)
	it("does not return an ID if the SDK never grants write access", function()
		catalog.withPrivateWriteAccessDo = function()
			return "aborted"
		end
		local id, err = Util.getGlobalPhotoIdForPhoto(newPhoto("one"))
		assert.is_nil(id)
		assert.matches("write access was not granted", err, 1, true)
	end)

	it("does not cache photos without a Lightroom UUID under a shared key", function()
		local photo = newPhoto("one")
		photo.raw.uuid = nil
		local id, err = Util.getGlobalPhotoIdForPhoto(photo)
		assert.is_nil(id)
		assert.matches("photo UUID are required", err, 1, true)
		assert.are.equal(0, catalog.writes)
	end)

	it("keeps virtual copies on the same backend identity with separate cache records", function()
		local master = newPhoto("master")
		local copy = newPhoto("copy")
		copy.raw.path, copy.formatted.fileName = master.raw.path, master.formatted.fileName
		copy.raw.isVirtualCopy = true
		assert.are.equal(Util.getGlobalPhotoIdForPhoto(master), Util.getGlobalPhotoIdForPhoto(copy))
		assert.are.equal(2, catalog.writes)
	end)

	it("rejects an invalid cache record without overwriting it", function()
		local photo = newPhoto("one")
		catalog.properties.photoIdentityV1_one = json:encode({ id = "", algorithm = "stable_meta_v1" })
		local id, err = Util.getGlobalPhotoIdForPhoto(photo)
		assert.is_nil(id)
		assert.matches("Invalid photo identity cache entry", err, 1, true)
		assert.are.equal(0, catalog.writes)
	end)

	it("reports identity errors in search, result lookup and missing-photo scans", function()
		newPhoto("one")
		catalog.withPrivateWriteAccessDo = function()
			error("write failed")
		end
		local messages = {}
		ErrorHandler.handleError = function(title, detail)
			messages[#messages + 1] = title .. ": " .. detail
		end
		local result, err = SearchIndexAPI.searchIndex("forest", photos)
		assert.is_nil(result)
		assert.matches("Could not resolve photo identity for search", err, 1, true)
		assert.same({}, SearchIndexAPI.findPhotosByPhotoIds({ "unknown" }))
		local ok = SearchIndexAPI.getMissingPhotosFromIndex({ enableEmbeddings = true }, scope)
		assert.is_false(ok)
		assert.are.equal(2, #messages)
		assert.matches("Check catalog write access", messages[1], 1, true)
		assert.matches("Check catalog write access", messages[2], 1, true)
		assert.are.equal(0, #requests)
	end)

	it("handles an empty catalog and backend claim failures without photo writes", function()
		local ok, err, result = SearchIndexAPI.claimPhotosForCatalog()
		assert.is_true(ok)
		assert.is_nil(err)
		assert.same({ claimed = 0, errors = 0 }, result)
		assert.are.equal(0, #requests)
		newPhoto("one")
		_G.LrHttp = {
			post = function()
				return '{"error":"unavailable"}', { status = 503 }
			end,
		}
		ok, err = SearchIndexAPI.claimPhotosForCatalog()
		assert.is_false(ok)
		assert.matches("unavailable", err, 1, true)
	end)
	it("sends cleanup as a complete inventory even above the old batch boundary", function()
		for i = 1, 5001 do
			newPhoto(tostring(i))
		end
		assert.is_true(SearchIndexAPI.syncCleanup())
		assert.are.equal(1, #requests)
		assert.are.equal(5001, #requests[1].body.photo_ids)
		assert.matches("/v1/photos/catalogs/cleanup", requests[1].url, 1, true)
	end)

	it("sends an empty cleanup inventory for an empty catalog", function()
		assert.is_true(SearchIndexAPI.syncCleanup())
		assert.are.equal(1, #requests)
		assert.same({}, requests[1].body.photo_ids)
	end)

	it("does not complete a claim migration when the backend reports partial errors", function()
		newPhoto("one")
		_G.LrHttp = {
			post = function()
				return '{"claimed":0,"errors":1}', { status = 200 }
			end,
		}
		local ok, err, result = SearchIndexAPI.claimPhotosForCatalog()
		assert.is_false(ok)
		assert.matches("could not claim 1 photos", err, 1, true)
		assert.are.equal(1, result.errors)
	end)
end)
