ErrorHandler = {}

function ErrorHandler.handleError(errorMessage, detailedInfo)
	-- Normalised once, here, so that neither the log nor the dialog can end up
	-- with a blank where the reason belongs. An empty string is truthy in Lua,
	-- so the `or` fallbacks that used to guard these never fired for a caller
	-- that passed one through.
	errorMessage = Util.errorText(errorMessage, "Something went wrong, but the cause was not reported.")
	detailedInfo =
		Util.errorText(detailedInfo, LOC("$$$/LrGeniusAI/ErrorHandler/noDetails=No additional details provided."))

	-- Log the error message
	log:error(LOC("$$$/LrGeniusAI/ErrorHandler/logError=Error: ^1", errorMessage))
	log:error(LOC("$$$/LrGeniusAI/ErrorHandler/logDetails=Details: ^1", detailedInfo))

	-- Show a dialog to the user with the error message
	-- LrDialogs.message(errorMessage, detailedInfo, "critical")
	ErrorHandler.customErrorDialog(errorMessage, detailedInfo)
end

function ErrorHandler.customErrorDialog(errorMessage, detailedInfo)
	local f = LrView.osFactory()
	local share = LrView.share

	-- Also normalised here: this is called directly as well as through
	-- handleError, and `title = nil` on a static_text has no fallback at all.
	errorMessage = Util.errorText(errorMessage, "Something went wrong, but the cause was not reported.")
	detailedInfo =
		Util.errorText(detailedInfo, LOC("$$$/LrGeniusAI/ErrorHandler/noDetails=No additional details provided."))

	local dialogView = f:column({
		f:row({
			f:static_text({
				title = LOC("$$$/LrGeniusAI/ErrorHandler/Error=Error"),
				alignment = "left",
				font = "<system/bold>",
				width = share("labelWidth"),
			}),
			f:static_text({
				title = errorMessage,
				alignment = "left",
				font = "<system/bold>",
			}),
		}),
		f:row({
			margin_top = 10,
			f:static_text({
				title = LOC("$$$/LrGeniusAI/ErrorHandler/Details=Details"),
				alignment = "left",
				width = share("labelWidth"),
			}),
			f:static_text({
				title = detailedInfo,
				alignment = "left",
				size = "small",
			}),
		}),
	})

	local result = LrDialogs.presentModalDialog({
		title = LOC("$$$/LrGeniusAI/ErrorHandler/Error=Error"),
		contents = dialogView,
		cancelVerb = LOC("$$$/LrGeniusAI/ErrorHandler/gatherLogs=Generate report"),
	})

	if result == "cancel" then
		LrTasks.startAsyncTask(function()
			Util.copyLogfilesToDesktop({ error = errorMessage, details = detailedInfo })
		end)
	end
end
