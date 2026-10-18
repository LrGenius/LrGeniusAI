-- AgentBridge.lua
-- The plugin's end of the agent bridge: lets an AI agent connected to the
-- backend's MCP endpoint (Claude Code, Claude Desktop, LM Studio, ...) work in
-- Lightroom. The backend half is server-rs/crates/lrg-api/src/agent_bridge.rs;
-- the commands themselves are in AgentCommands.lua.
--
-- Two channels, on purpose:
--
--   * An LrSocket in receive mode is a doorbell. The backend connects to it
--     and writes "wake" when a command is queued, and "ping" every few seconds
--     so a dead backend can be told from a quiet one. Nothing on it is acted
--     on except by asking the backend over HTTP for queued work, so whatever
--     else writes to the port can at most make us ask for work there is not.
--
--   * HTTP carries everything with content: register, claim commands, post
--     results and progress. It is how the plugin already talks to the backend.
--
-- No send-mode socket: lightroom-mcp found that LrSocket's send mode does not
-- reliably notice a dropped client on Windows and keeps writing into the void
-- (their issues #110 and #134). And the socket only speeds things up: without
-- it — bind failed, connection dropped — the loop asks every second instead.
--
-- Commands run one at a time on this task. Parallel LrTasks have caused
-- trouble in this plugin before, especially on Windows.
--
-- The bridge starts with Lightroom (unless turned off in the Plug-in Manager)
-- and stays out of sight while no agent works. While one does, a progress bar
-- in Lightroom's activity area says what it is doing, and its X is the user's
-- off switch: it stops the bridge for the rest of the session.
--
-- MCP clients need the agent token, which the backend makes and the plugin
-- keeps in the system's password storage (Secrets.getMcpToken). It goes to
-- the backend with every registration, so the agents' setup survives
-- restarts; "New token" in the Plug-in Manager asks the backend for another.

AgentBridge = {}

local LrSocket = import("LrSocket")

-- Kept on _G, not in this module: "Reload Plug-in" runs Init.lua again in the
-- same Lua state while the previous instance's loop may still be winding down
-- (lightroom-mcp issues #121 and #137). Every start takes a new instance
-- number; a loop that sees a newer one exits.
if not _G.LrGeniusAgentBridgeState then
	_G.LrGeniusAgentBridgeState = { running = false, instance = 0, commandsDone = 0 }
end
local state = _G.LrGeniusAgentBridgeState

local REGISTER_RETRY_SECONDS = 3
-- How long the activity bar stays after the agent's last request, so a run
-- of requests shows as one bar rather than flickering.
local ACTIVITY_LINGER_SECONDS = 8
-- How long a fresh socket may take to report its port before it is replaced.
local PORT_WAIT_SECONDS = 5
-- How often to ask for work when the socket is connected (a safety net for a
-- lost "wake") and when it is not (the fallback).
local CLAIM_INTERVAL_CONNECTED = 5
local CLAIM_INTERVAL_POLLING = 1
-- Progress posts are throttled; a command that reports per photo would
-- otherwise make one HTTP call per photo.
local PROGRESS_INTERVAL_SECONDS = 0.5

--- True when the agent may run code inside the plugin. Needs both the
--- developer switch and a development install: a released plugin never
--- offers it, whatever the preferences say.
function AgentBridge.devToolsEnabled()
	local path = tostring(_PLUGIN and _PLUGIN.path or "")
	return prefs.agentBridgeDevTools == true and path:lower():match("%.lrdevplugin[/\\]?$") ~= nil
end

function AgentBridge.isDevInstall()
	local path = tostring(_PLUGIN and _PLUGIN.path or "")
	return path:lower():match("%.lrdevplugin[/\\]?$") ~= nil
end

function AgentBridge.isRunning()
	return state.running == true
end

--- What the Plug-in Manager shows.
function AgentBridge.status()
	return {
		running = state.running == true,
		registered = state.token ~= nil,
		socketConnected = state.socketConnected == true,
		commandsDone = state.commandsDone or 0,
		lastCommand = state.lastCommand,
		lastError = state.lastError,
		stoppedByUser = state.stoppedByUser == true,
	}
end

--- Asks the backend for a new agent token at the next registration; every
--- MCP client set up with the old one is locked out.
function AgentBridge.rotateMcpToken()
	state.rotateToken = true
	state.reregister = true
end

--- The bridge's settings changed: register again so the backend knows.
function AgentBridge.refreshRegistration()
	state.reregister = true
end

--- Asks the running bridge to stop; it unregisters and closes its socket.
function AgentBridge.stop()
	state.stopRequested = true
end

--- Called from Init.lua, which runs on load and on "Reload Plug-in" (never on
--- a Plug-in Manager render). A loop left over from before the reload sees the
--- new instance number and exits.
function AgentBridge.resetForReload()
	state.instance = (state.instance or 0) + 1
	state.running = false
	state.stopRequested = false
	state.token = nil
	state.socketConnected = false
end

--- Lightroom is quitting: tell the backend now, while HTTP still works.
function AgentBridge.shutdown()
	state.stopRequested = true
	local token = state.token
	if token then
		state.token = nil
		LrTasks.pcall(SearchIndexAPI.agentUnregister, token)
	end
end

local function pluginVersion()
	return string.format("%s.%s.%s", tostring(Info.MAJOR), tostring(Info.MINOR), tostring(Info.REVISION))
end

-- Who owns the bridge. "Reload Plug-in" gives the reloaded plugin a fresh Lua
-- environment while a loop the old instance started can keep running (seen
-- with LrC 15.6): the two then take turns replacing each other's backend
-- session. Every start writes its start time here, in the prefs every
-- instance of the plugin shares, and a loop that no longer finds its own
-- exits. The backend enforces the same rule from its side: it refuses a
-- registration from a bridge started before the newest one (409 superseded).
local OWNER_PREF = "agentBridgeOwner"

local function registrationBody(session)
	local catalog = LrApplication.activeCatalog()
	local path = catalog:getPath()
	local catalogId = Util.getCatalogIdentifier()
	return {
		port = session.port,
		previous_token = session.token,
		plugin_version = pluginVersion(),
		lightroom_version = LrApplication.versionString(),
		catalog_path = path,
		catalog_name = LrPathUtils.removeExtension(LrPathUtils.leafName(path)),
		catalog_id = catalogId,
		allow_changes = prefs.agentBridgeAllowChanges ~= false,
		dev_tools = AgentBridge.devToolsEnabled(),
		platform = WIN_ENV and "windows" or "mac",
		mcp_token = Secrets.getMcpToken(),
		rotate_mcp_token = state.rotateToken == true,
		started_at = session.startedAt,
	}
end

local function runLoop(context, instance, startedAt)
	-- Shown only while an agent works; see beginActivity.
	local activity = nil
	local lastActivity = 0
	local owner = tostring(startedAt)

	local session = {
		startedAt = startedAt,
		token = nil,
		pingInterval = 5,
		lastRegister = -math.huge,
		lastClaim = -math.huge,
		socket = nil,
		socketGen = 0,
		port = nil,
		portChanged = false,
		nextBindAt = 0,
		bindDeadline = 0,
		bindAttempts = 0,
		socketConnected = false,
		lastSocketMessage = 0,
		needReconnect = false,
		wake = false,
		cache = AgentCommands.newCache(),
	}

	local function live()
		if activity and activity:isCanceled() and not state.stopRequested then
			-- The user pressed the X on the activity bar: stop agent control.
			state.stopRequested = true
			state.stoppedByUser = true
			LrDialogs.showBezel("AI agent stopped. Plug-in Manager > LrGeniusAI > AI Agents starts it again.")
		end
		if prefs[OWNER_PREF] ~= owner and not state.stopRequested then
			log:info("Agent bridge: a newer bridge took over (the plugin was reloaded); this one stops")
			state.stopRequested = true
		end
		return not state.stopRequested and state.instance == instance
	end

	local function beginActivity()
		if not activity then
			activity = LrProgressScope({
				title = "LrGeniusAI: AI agent at work",
				functionContext = context,
			})
			activity:setCancelable(true)
		end
		lastActivity = LrDate.currentTime()
	end

	local function endActivity()
		if activity then
			activity:done()
			activity = nil
		end
	end

	local function caption(text)
		if activity then
			activity:setCaption(text)
		end
	end

	local function bindSocket()
		session.socketGen = session.socketGen + 1
		local gen = session.socketGen
		local function current()
			return session.socketGen == gen
		end
		local ok, socket = LrTasks.pcall(function()
			return LrSocket.bind({
				functionContext = context,
				plugin = _PLUGIN,
				port = 0, -- the OS picks; registration tells the backend
				mode = "receive",
				onConnecting = function(_, port)
					if current() and session.port ~= port then
						session.port = port
						session.portChanged = true
						session.bindAttempts = 0
					end
				end,
				onConnected = function()
					if current() then
						session.socketConnected = true
						session.lastSocketMessage = LrDate.currentTime()
					end
				end,
				-- Runs where nothing may yield (no HTTP, no sleep): only flags.
				onMessage = function(_, message)
					if not current() then
						return
					end
					session.lastSocketMessage = LrDate.currentTime()
					if type(message) == "string" and message:find("wake", 1, true) then
						session.wake = true
					end
				end,
				onClosed = function()
					if current() then
						session.socketConnected = false
						session.needReconnect = true
					end
				end,
				onError = function(_, err)
					if not current() then
						return
					end
					local text = tostring(err)
					-- The accept loop reports these while no client is attached.
					if text == "timeout" or text:find("failed to open", 1, true) then
						if not session.socketConnected then
							session.needReconnect = true
						end
					else
						log:warn("Agent bridge socket error: " .. text)
						session.socketConnected = false
						session.needReconnect = true
					end
				end,
			})
		end)
		if ok and socket then
			session.socket = socket
		else
			session.bindAttempts = session.bindAttempts + 1
			session.nextBindAt = LrDate.currentTime() + math.min(60, PORT_WAIT_SECONDS * 2 ^ session.bindAttempts)
			log:warn("Agent bridge: could not open the socket (" .. tostring(socket) .. "); asking over HTTP instead")
		end
	end

	local function closeSocket()
		-- Bumped first: callbacks that fire while closing see themselves as
		-- stale (lightroom-mcp issue #110).
		session.socketGen = session.socketGen + 1
		if session.socket then
			LrTasks.pcall(function()
				session.socket:close()
			end)
		end
		session.socket = nil
		session.socketConnected = false
		session.port = nil
	end

	local function register()
		session.lastRegister = LrDate.currentTime()
		local response, err, status, body = SearchIndexAPI.agentRegister(registrationBody(session))
		if status == 409 and type(body) == "table" and body.superseded then
			log:info("Agent bridge: the backend has a newer bridge registered; this one stops")
			state.stopRequested = true
			return
		end
		if response and response.token then
			session.token = response.token
			state.token = response.token
			session.pingInterval = tonumber(response.ping_interval_seconds) or session.pingInterval
			session.portChanged = false
			session.lastSocketMessage = LrDate.currentTime()
			state.lastError = nil
			state.rotateToken = false
			-- Kept for next time: AI apps are set up with this token once.
			if type(response.mcp_token) == "string" and response.mcp_token ~= Secrets.getMcpToken() then
				local stored, problem = Secrets.setMcpToken(response.mcp_token)
				if not stored and not state.tokenProblemShown then
					state.tokenProblemShown = true
					ErrorHandler.handleError("The agent token could not be stored", problem)
				end
			end
			log:info(
				"Agent bridge registered (session "
					.. tostring(response.session)
					.. ", port "
					.. tostring(session.port)
					.. ")"
			)
		else
			state.lastError = "The LrGeniusAI backend did not answer: " .. tostring(err)
		end
	end

	local function makeEnv(command)
		local lastPost = -math.huge
		local cancelled = false
		return {
			allowChanges = prefs.agentBridgeAllowChanges ~= false,
			devTools = AgentBridge.devToolsEnabled(),
			cache = session.cache,
			context = context,
			--- Reports progress to the agent; returns true once the agent has
			--- given up on this command.
			progress = function(done, total, message)
				local now = LrDate.currentTime()
				if now - lastPost < PROGRESS_INTERVAL_SECONDS and done ~= total then
					return cancelled
				end
				lastPost = now
				if message then
					caption(message .. " (" .. tostring(done) .. "/" .. tostring(total or "?") .. ")")
				end
				lastActivity = now
				local reply = SearchIndexAPI.agentPostProgress(session.token, command.id, {
					done = done,
					total = total,
					message = message,
				})
				cancelled = cancelled or (type(reply) == "table" and reply.cancel == true)
				return cancelled or not live()
			end,
		}
	end

	local function claimAndRun()
		session.lastClaim = LrDate.currentTime()
		local response, err, status, body = SearchIndexAPI.agentClaimCommands(session.token)
		if not response then
			if status == 401 or (type(body) == "table" and body.reregister) then
				-- The backend restarted and forgot us: register again.
				session.token = nil
				state.token = nil
			else
				state.lastError = "Could not ask the backend for work: " .. tostring(err)
			end
			return
		end
		for _, command in ipairs(response.commands or {}) do
			local reply
			if live() then
				beginActivity()
				caption(AgentCommands.describe(command))
				reply = AgentCommands.run(command, makeEnv(command))
				lastActivity = LrDate.currentTime()
				caption("Waiting for the agent's next request...")
			else
				-- Claimed but the user just stopped the bridge: say so rather
				-- than leave the agent waiting for its timeout.
				reply = { ok = false, error = "The user stopped agent control in Lightroom." }
			end
			local posted, postErr = SearchIndexAPI.agentPostResult(session.token, command.id, reply)
			if not posted then
				state.lastError = "Could not report a result to the backend: " .. tostring(postErr)
			end
			state.commandsDone = (state.commandsDone or 0) + 1
			state.lastCommand = tostring(command.action)
				.. (reply.ok and "" or " (failed)")
				.. " at "
				.. LrDate.timeToUserFormat(LrDate.currentTime(), "%H:%M:%S")
		end
	end

	while live() do
		local now = LrDate.currentTime()

		if session.needReconnect and session.socket then
			session.needReconnect = false
			LrTasks.pcall(function()
				session.socket:reconnect()
			end)
		end

		-- Connected but silent for several pings: on Windows a backend that
		-- went away does not always close the socket. A fresh socket (new
		-- port, re-registered) recovers either way.
		if session.socketConnected and now - session.lastSocketMessage > 3 * session.pingInterval + 2 then
			log:info("Agent bridge: the backend went quiet; opening a fresh socket")
			closeSocket()
			session.nextBindAt = now
		end

		if state.reregister then
			state.reregister = false
			session.portChanged = true
		end
		local registerDue = (not session.token) or session.portChanged
		if registerDue and now - session.lastRegister >= (session.token and 0.5 or REGISTER_RETRY_SECONDS) then
			register()
		end

		-- The doorbell socket, bound only once the backend has answered: a
		-- socket bound while Lightroom is still starting up never reports its
		-- port (seen with LrC 15.6 on macOS), and nothing could connect to it
		-- before the backend is up anyway. One that has not reported a port
		-- within a few seconds is replaced, with growing pauses; the bridge
		-- polls over HTTP in the meantime.
		if session.token and not session.socket and now >= session.nextBindAt then
			bindSocket()
			session.bindDeadline = now + PORT_WAIT_SECONDS
		elseif session.socket and not session.port and now > session.bindDeadline then
			log:info("Agent bridge: the socket did not report a port; opening a new one")
			closeSocket()
			session.bindAttempts = session.bindAttempts + 1
			session.nextBindAt = now + math.min(60, PORT_WAIT_SECONDS * 2 ^ session.bindAttempts)
		end

		local interval = session.socketConnected and CLAIM_INTERVAL_CONNECTED or CLAIM_INTERVAL_POLLING
		if session.token and (session.wake or now - session.lastClaim >= interval) then
			session.wake = false
			claimAndRun()
		end

		state.socketConnected = session.socketConnected
		if activity and LrDate.currentTime() - lastActivity > ACTIVITY_LINGER_SECONDS then
			endActivity()
		end
		LrTasks.sleep(0.1)
	end

	-- Stopping: the backend learns at once, so a waiting agent gets "stopped"
	-- instead of a timeout. Not when a newer bridge took over: the session
	-- may already be its own.
	if session.token and state.instance == instance and prefs[OWNER_PREF] == owner then
		LrTasks.pcall(SearchIndexAPI.agentUnregister, session.token)
	end
	closeSocket()
	endActivity()
	if state.instance == instance then
		state.token = nil
		state.socketConnected = false
		state.running = false
		log:info("Agent bridge stopped")
	end
end

--- Starts the bridge. Returns false when it is already running.
function AgentBridge.start()
	if state.running then
		return false
	end
	state.running = true
	state.stopRequested = false
	state.stoppedByUser = false
	state.instance = (state.instance or 0) + 1
	local instance = state.instance
	local startedAt = LrDate.currentTime()
	prefs[OWNER_PREF] = tostring(startedAt)
	-- A context of its own: a task started with startAsyncTask from Init.lua
	-- dies when Init.lua's context ends (lightroom-mcp issue #128).
	LrFunctionContext.postAsyncTaskWithContext("LrGeniusAI agent bridge", function(context)
		context:addCleanupHandler(function()
			if state.instance == instance then
				state.running = false
				state.token = nil
				state.socketConnected = false
			end
		end)
		local ok, err = LrTasks.pcall(runLoop, context, instance, startedAt)
		if not ok then
			log:error("Agent bridge stopped by an error: " .. tostring(err))
			ErrorHandler.handleError("The AI agent bridge stopped", tostring(err))
		end
	end)
	return true
end

return AgentBridge
