-- PluginTests.lua
-- The plugin's tests that need the real thing: Lightroom's own Lua, the SDK,
-- an open catalog and the running backend — everything busted's stubs cannot
-- reach. Two ways in: Help > "Developer: Run Automated Tests..." in
-- Lightroom, and the agent bridge's run_plugin_tests tool, which is what lets
-- an AI agent (or a script talking MCP) run them after every change.
--
-- Suites marked `writes` change the catalog. They only run when asked to and
-- when the user allows agent changes, they work on one photo (the selected
-- one, else the first in the catalog), and they put back everything they
-- change; what cannot be put back (develop history steps, the test keyword
-- itself) is named in the result.

PluginTests = {}

local suites = {}

--- Declares a suite; returns the function that adds a test to it.
local function suite(name, options)
	local s = { name = name, writes = options and options.writes or false, tests = {} }
	suites[#suites + 1] = s
	return function(testName, fn)
		s.tests[#s.tests + 1] = { name = testName, fn = fn }
	end
end

local SKIP = {}

--- Ends the current test as skipped, with a reason.
local function skip(reason)
	error({ [SKIP] = true, reason = reason }, 0)
end

local function fail(message)
	error(message, 0)
end

local function assertEqual(expected, actual, message)
	if expected ~= actual then
		fail(string.format("%s (expected %s, got %s)", message, tostring(expected), tostring(actual)))
	end
end

local function assertTrue(condition, message)
	if not condition then
		fail(message)
	end
end

PluginTests.assertEqual = assertEqual
PluginTests.assertTrue = assertTrue
PluginTests.skip = skip

--- Runs one bridge command the way the bridge would, and fails the test on
--- an error reply.
local function command(ctx, action, args)
	local reply = AgentCommands.run({ id = 0, action = action, args = args or {} }, ctx.env)
	if not reply.ok then
		fail(action .. " failed: " .. tostring(reply.error))
	end
	return reply.result, reply.warnings
end

--- The photo the catalog tests use: the active one, else the first photo.
local function testPhoto(ctx)
	if ctx.photo == nil then
		local catalog = LrApplication.activeCatalog()
		ctx.photo = catalog:getTargetPhoto() or catalog:getAllPhotos()[1] or false
	end
	if not ctx.photo then
		skip("the catalog has no photos")
	end
	return ctx.photo
end

local function testPhotoId(ctx)
	local photo = testPhoto(ctx)
	local id, err = SearchIndexAPI.getPhotoIdForPhoto(photo)
	if not id then
		skip("the test photo has no LrGeniusAI photo ID: " .. tostring(err))
	end
	ctx.env.cache.byId[id] = photo
	return id
end

---------------------------------------------------------------------------
-- Lightroom's Lua: what the plugin relies on actually exists here
---------------------------------------------------------------------------

local test = suite("environment")

test("Util string helpers", function()
	local parts = Util.string_split("a,b,c", ",")
	assertEqual(3, #parts, "string_split returns three parts")
	assertEqual("a", parts[1], "first part")
	assertEqual("hello", Util.trim("  hello  "), "trim removes spaces")
	assertEqual("hello", Util.trim("hello\n"), "trim removes newlines")
end)

test("JSON round trip", function()
	local decoded = JSON:decode('["a", "b", "c"]')
	assertEqual(3, #decoded, "array length")
	assertEqual("b", decoded[2], "second element")
	local encoded = JSON:encode({ success = true, meta = { value = 1 } })
	assertTrue(encoded:find("success", 1, true) ~= nil, "encoded object keeps its key")
end)

test("loadstring is available (developer commands need it)", function()
	if type(loadstring) ~= "function" then
		skip("this Lightroom has no loadstring; eval_lua and reload_plugin_modules cannot work")
	end
	local chunk = loadstring("return 40 + 2")
	assertEqual(42, chunk(), "a compiled chunk runs")
end)

test("agent command helpers", function()
	local parts = AgentCommands.splitPath(" Places > Hamburg ")
	assertEqual(2, #parts, "keyword path splits in two")
	assertEqual("Hamburg", parts[2], "parts are trimmed")
	local desc = AgentCommands.buildSearchDesc({ rating_min = 3, flag = "pick" })
	assertEqual("intersect", desc.combine, "two criteria are intersected")
	local cyclic = {}
	cyclic.self = cyclic
	assertEqual("<cycle>", AgentCommands.toJsonSafe(cyclic).self, "cycles are cut")
end)

---------------------------------------------------------------------------
-- The backend
---------------------------------------------------------------------------

test = suite("backend")

test("backend answers ping", function()
	assertTrue(SearchIndexAPI.pingServer(), "the backend is reachable at " .. tostring(prefs.backendServerUrl))
end)

test("backend and plugin versions are compatible", function()
	local result = SearchIndexAPI.checkVersionCompatibility()
	if type(result) ~= "table" then
		fail("the version check did not answer")
	end
	assertTrue(result.compatible ~= false, "versions compatible: " .. tostring(result.reason or result.error))
end)

test("index statistics are readable", function()
	local stats = SearchIndexAPI.getStats()
	assertTrue(type(stats) == "table", "stats come back")
	assertTrue(stats.photos ~= nil, "stats have a photos section")
end)

test("agent bridge status is readable", function()
	local status, err = SearchIndexAPI.getAgentBridgeStatus()
	assertTrue(type(status) == "table", "GET /v1/agent/bridge answers: " .. tostring(err))
	if AgentBridge.isRunning() then
		assertTrue(status.connected == true, "the backend counts this running bridge as connected")
	end
end)

test("the agent token is kept in the password storage", function()
	if not AgentBridge.status().registered then
		skip("the agent bridge is not connected to the backend")
	end
	local token = Secrets.getMcpToken()
	assertTrue(#token >= 32, "a token of at least 32 characters is kept")
end)

---------------------------------------------------------------------------
-- Catalog and photo identity
---------------------------------------------------------------------------

test = suite("catalog")

test("the catalog is open and has a path", function()
	local catalog = LrApplication.activeCatalog()
	assertTrue(catalog ~= nil, "a catalog is open")
	assertTrue(type(catalog:getPath()) == "string" and catalog:getPath() ~= "", "the catalog has a path")
end)

test("the catalog has a stable identifier", function()
	local id = Util.getCatalogIdentifier()
	assertTrue(type(id) == "string" and id:match("^cat_") ~= nil, "catalog id looks like cat_...")
	assertEqual(id, Util.getCatalogIdentifier(), "the identifier is stable")
end)

test("a photo ID resolves back to the same photo", function(ctx)
	local photo = testPhoto(ctx)
	local id = testPhotoId(ctx)
	local fresh = { allowChanges = false, devTools = false, cache = AgentCommands.newCache() }
	local photos, missing = AgentCommands.resolvePhotos(fresh, { id })
	assertEqual(0, #missing, "the ID is found")
	assertEqual(photo.localIdentifier, photos[1].localIdentifier, "it is the same photo")
end)

---------------------------------------------------------------------------
-- Bridge commands that only read
---------------------------------------------------------------------------

test = suite("agent_commands")

test("status describes Lightroom", function(ctx)
	local result = command(ctx, "status")
	assertTrue(type(result.catalog) == "table", "status has the catalog")
	assertTrue((result.catalog.photo_count or -1) >= 0, "status counts the photos")
	assertTrue(type(result.lightroom_version) == "string", "status has the Lightroom version")
end)

test("get_photos returns the selection", function(ctx)
	local result = command(ctx, "get_photos", { scope = "selection", limit = 10 })
	assertTrue(type(result.photo_ids) == "table", "photo_ids is a list")
end)

test("photo_info describes a photo", function(ctx)
	local id = testPhotoId(ctx)
	local result = command(ctx, "photo_info", { photo_ids = { id } })
	assertEqual(1, #result.photos, "one photo described")
	assertTrue(result.photos[1].file_name ~= nil, "it has a file name")
end)

test("get_develop_settings reads settings", function(ctx)
	local id = testPhotoId(ctx)
	local result = command(ctx, "get_develop_settings", { photo_id = id, keys = { "Exposure2012" } })
	assertTrue(type(result.settings) == "table", "settings come back")
end)

test("list_collections walks the tree", function(ctx)
	local result = command(ctx, "list_collections")
	assertTrue(type(result.collections) == "table", "collections is a list")
end)

test("find_photos runs a catalog query", function(ctx)
	local result = command(ctx, "find_photos", { criteria = { rating_min = 0 }, limit = 5 })
	assertTrue(type(result.photo_ids) == "table", "photo_ids is a list")
	assertTrue(#result.photo_ids <= 5, "the limit holds")
end)

test("render_preview renders a JPEG", function(ctx)
	local id = testPhotoId(ctx)
	local result = command(ctx, "render_preview", { photo_id = id, size = 256 })
	assertTrue(type(result.path) == "string" and LrFileUtils.exists(result.path), "the preview file exists")
	LrFileUtils.delete(result.path)
end)

test("read_log returns lines", function(ctx)
	local result = command(ctx, "read_log", { lines = 5 })
	assertTrue(type(result.lines) == "table", "lines is a list")
end)

test("unknown commands are answered, not raised", function(ctx)
	local reply = AgentCommands.run({ id = 0, action = "no_such_command", args = {} }, ctx.env)
	assertEqual(false, reply.ok, "the reply says it failed")
	assertTrue(type(reply.error) == "string", "with a message")
end)

test("a read-only bridge refuses changes", function(ctx)
	local readOnly = { allowChanges = false, devTools = false, cache = ctx.env.cache }
	local reply = AgentCommands.run({ id = 0, action = "update_photos", args = { photo_ids = {} } }, readOnly)
	assertEqual("changes", reply.refused, "update_photos is refused as a change")
end)

---------------------------------------------------------------------------
-- Bridge commands that change the catalog, undone afterwards
---------------------------------------------------------------------------

test = suite("catalog_writes", { writes = true })

test("update_photos changes rating, flag, label and title, then restores them", function(ctx)
	local id = testPhotoId(ctx)
	local before = command(ctx, "photo_info", { photo_ids = { id } }).photos[1]
	command(ctx, "update_photos", {
		photo_ids = { id },
		rating = 3,
		flag = "pick",
		color_label = "green",
		title = "LrGeniusAI test title",
	})
	local after = command(ctx, "photo_info", { photo_ids = { id } }).photos[1]
	-- Restore before asserting, so a failed check does not leave the change.
	command(ctx, "update_photos", {
		photo_ids = { id },
		rating = before.rating or 0,
		flag = before.flag or "none",
		color_label = before.color_label or "none",
		title = before.title or "",
	})
	assertEqual(3, after.rating, "rating was set")
	assertEqual("pick", after.flag, "flag was set")
	assertEqual("green", after.color_label, "label was set")
	assertEqual("LrGeniusAI test title", after.title, "title was set")
	local restored = command(ctx, "photo_info", { photo_ids = { id } }).photos[1]
	assertEqual(before.rating or 0, restored.rating, "rating restored")
	assertEqual(before.flag, restored.flag, "flag restored")
end)

test("keywords are added and removed by path", function(ctx)
	local id = testPhotoId(ctx)
	command(ctx, "update_photos", { photo_ids = { id }, add_keywords = { "LrGeniusAI Tests > Agent Bridge" } })
	local with = command(ctx, "photo_info", { photo_ids = { id } }).photos[1]
	command(ctx, "update_photos", { photo_ids = { id }, remove_keywords = { "LrGeniusAI Tests > Agent Bridge" } })
	local without = command(ctx, "photo_info", { photo_ids = { id } }).photos[1]
	assertTrue(tostring(with.keywords):find("Agent Bridge", 1, true) ~= nil, "the keyword was added")
	assertTrue(tostring(without.keywords or ""):find("Agent Bridge", 1, true) == nil, "the keyword was removed")
	ctx.leftovers[#ctx.leftovers + 1] =
		'The keyword "LrGeniusAI Tests > Agent Bridge" stays in the catalog (the SDK cannot delete keywords).'
end)

test("a collection is filled and emptied", function(ctx)
	local id = testPhotoId(ctx)
	local args = { photo_ids = { id }, collection = "Agent Bridge Test", collection_set = "LrGeniusAI Tests" }
	command(ctx, "collection", args)
	args.op = "remove"
	local removed = command(ctx, "collection", args)
	assertEqual(1, removed.removed, "the photo was removed again")
	ctx.leftovers[#ctx.leftovers + 1] = 'The empty collection "LrGeniusAI Tests > Agent Bridge Test" stays.'
end)

test("develop settings apply relatively and are restored", function(ctx)
	local id = testPhotoId(ctx)
	local before = command(ctx, "get_develop_settings", { photo_id = id, keys = { "Exposure2012" } })
	local original = before.settings.Exposure2012
	if type(original) ~= "number" then
		skip("the test photo has no Exposure2012 (not process version 2012 or later)")
	end
	-- Only the snapshots this test makes are removed afterwards; earlier
	-- agent edits keep theirs.
	local photo = testPhoto(ctx)
	local existing = {}
	for _, snapshot in ipairs(photo:getDevelopSnapshots() or {}) do
		existing[snapshot.snapshotID] = true
	end
	local applied = command(ctx, "apply_develop_settings", {
		photo_ids = { id },
		settings = { Exposure2012 = 0.25 },
		relative = true,
	})
	local changed = command(ctx, "get_develop_settings", { photo_id = id, keys = { "Exposure2012" } })
	command(ctx, "apply_develop_settings", { photo_ids = { id }, settings = { Exposure2012 = original } })
	assertTrue(math.abs(changed.settings.Exposure2012 - (original + 0.25)) < 0.001, "exposure went up by 0.25")

	-- The snapshots the two calls left behind.
	LrApplication.activeCatalog():withWriteAccessDo("LrGeniusAI test cleanup", function()
		for _, snapshot in ipairs(photo:getDevelopSnapshots() or {}) do
			if not existing[snapshot.snapshotID] and tostring(snapshot.name):find("^Before LrGeniusAI agent edit") then
				photo:deleteDevelopSnapshot(snapshot.snapshotID)
			end
		end
	end, { timeout = 10 })
	assertTrue(applied.updated == 1, "one photo was updated")
	ctx.leftovers[#ctx.leftovers + 1] = "The test photo's develop history has two more steps (changed and restored)."
end)

---------------------------------------------------------------------------
-- Runner
---------------------------------------------------------------------------

function PluginTests.suiteNames()
	local names = {}
	for _, s in ipairs(suites) do
		names[#names + 1] = s.name .. (s.writes and " (changes the catalog)" or "")
	end
	return names
end

--- Runs the tests. `options`: suite (one suite's name), filter (text in the
--- test name), includeWrites, allowChanges, progress(done, total, message),
--- env (the bridge's command environment; a fresh one by default).
function PluginTests.run(options)
	options = options or {}
	local env = options.env
		or { allowChanges = options.allowChanges ~= false, devTools = false, cache = AgentCommands.newCache() }
	local ctx = { env = env, leftovers = {} }

	local selected = {}
	local notes = {}
	for _, s in ipairs(suites) do
		local wanted = (not options.suite or options.suite == s.name)
		if wanted and s.writes and not options.includeWrites then
			if options.suite == s.name then
				notes[#notes + 1] = s.name .. " changes the catalog; pass include_catalog_writes to run it."
			end
			wanted = false
		end
		if wanted and s.writes and not env.allowChanges then
			notes[#notes + 1] = s.name .. " skipped: the user does not allow agent changes."
			wanted = false
		end
		if wanted then
			for _, t in ipairs(s.tests) do
				if not options.filter or t.name:lower():find(tostring(options.filter):lower(), 1, true) then
					selected[#selected + 1] = { suite = s.name, test = t }
				end
			end
		end
	end
	if options.suite and #selected == 0 and #notes == 0 then
		error(
			'There is no test suite "'
				.. tostring(options.suite)
				.. '". Suites: '
				.. table.concat(PluginTests.suiteNames(), ", ")
		)
	end

	local results = {}
	local passed, failed, skipped = 0, 0, 0
	local started = LrDate.currentTime()
	for index, entry in ipairs(selected) do
		if options.progress and options.progress(index - 1, #selected, "Testing: " .. entry.test.name) then
			notes[#notes + 1] = "Stopped early: the agent cancelled the run."
			break
		end
		local t0 = LrDate.currentTime()
		local ok, err = LrTasks.pcall(entry.test.fn, ctx)
		local result = {
			suite = entry.suite,
			name = entry.test.name,
			ms = math.floor((LrDate.currentTime() - t0) * 1000),
		}
		if ok then
			result.ok = true
			passed = passed + 1
		elseif type(err) == "table" and err[SKIP] then
			result.skipped = err.reason
			skipped = skipped + 1
		else
			result.ok = false
			result.error = tostring(err)
			failed = failed + 1
			log:error("Plugin test failed: " .. entry.suite .. " / " .. entry.test.name .. ": " .. result.error)
		end
		results[#results + 1] = result
	end
	return {
		passed = passed,
		failed = failed,
		skipped = skipped,
		duration_ms = math.floor((LrDate.currentTime() - started) * 1000),
		suites = PluginTests.suiteNames(),
		results = results,
		notes = (#notes > 0) and notes or nil,
		leftovers = (#ctx.leftovers > 0) and ctx.leftovers or nil,
	}
end

return PluginTests
