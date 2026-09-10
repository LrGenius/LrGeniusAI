-- Analyze Selection with LrGeniusAI
--
-- Installed into ~/Library/Scripts/Capture One Scripts/LrGeniusAI/ by
-- install.sh, where Capture One picks it up and shows it in its Scripts menu.
--
-- This script is deliberately thin. It does not read the selection, does not
-- talk to the backend and does not decide anything: it starts the companion
-- program (lrgenius-c1) and gets out of the way. Everything else -- reading
-- the selected variants over AppleScript, indexing, writing results back --
-- happens in the companion. The only job left here is to start it and to tell
-- the user, in a dialog, when that did not work.
--
-- If you do not know AppleScript: "on run" is what Capture One calls when the
-- menu item is chosen, "on <name>(...)" declares a function ("handler"),
-- "do shell script" runs a shell command, and "display dialog" shows a
-- message box.

-- What to run. Changing these two lines is what makes the three scripts in
-- this folder different from each other.
property companionArguments : {"analyze", "--source", "selection"}
property actionTitle : "Analyze Selection"

on run
	launchCompanion(companionArguments, actionTitle)
end run


-- ===================================================================
-- Shared launcher block.
--
-- Every script in this folder carries its own copy on purpose: a script in the
-- Scripts menu has to keep working when it is the only file someone copied
-- over, so it may not depend on a script library being installed as well.
-- When you change one copy, change all three.
-- ===================================================================

-- Starts the companion, or explains to the user why it could not start.
on launchCompanion(companionArguments, actionTitle)
	try
		do shell script shellProgramFor(companionArguments)
	on error errorText number errorNumber
		reportProblem(errorNumber, errorText, actionTitle)
	end try
end launchCompanion


-- Builds the small shell program that finds the companion binary, starts it
-- detached, and reports back through its exit status:
--
--   0   the companion is running (or finished successfully within a moment)
--   65  LRGENIUS_C1_BIN is set but does not point at an executable file
--   66  no companion binary on any of the known paths
--   70  the companion started but exited immediately -- stderr carries the
--       exit status and the tail of its log
--
-- Detaching matters: "do shell script" waits for the command to finish, and
-- Capture One waits for the script, so a companion started in the foreground
-- would freeze the whole application for as long as the analysis runs.
-- "nohup ... &" plus redirected input and output is what lets the shell return
-- straight away while the companion keeps running.
on shellProgramFor(companionArguments)
	-- Turn {"analyze", "--source", "selection"} into one shell-safe string:
	--  'analyze' '--source' 'selection'
	set argumentText to ""
	repeat with anArgument in companionArguments
		set argumentText to argumentText & " " & quoted form of (anArgument as text)
	end repeat

	set programLines to {}

	-- Fail on unset variables rather than on an empty path silently.
	set end of programLines to "set -u"

	-- Where the companion's start-up output goes. Same place the rest of
	-- LrGeniusAI logs on macOS. /dev/null if that is somehow not writable --
	-- losing the log is better than failing to start.
	set end of programLines to "lrg_log_dir=\"$HOME/Library/Logs/LrGeniusAI\""
	set end of programLines to "mkdir -p \"$lrg_log_dir\" 2>/dev/null || true"
	set end of programLines to "lrg_log=\"$lrg_log_dir/lrgenius-c1-launch.log\""
	set end of programLines to "[ -w \"$lrg_log_dir\" ] || lrg_log=/dev/null"

	-- Keep the log from growing forever: one rotation at 1 MiB.
	set end of programLines to "if [ -f \"$lrg_log\" ] && [ \"$(stat -f%z \"$lrg_log\" 2>/dev/null || echo 0)\" -gt 1048576 ]; then"
	set end of programLines to "  mv -f \"$lrg_log\" \"$lrg_log.1\" 2>/dev/null || true"
	set end of programLines to "fi"

	-- Find the companion. LRGENIUS_C1_BIN wins when it is set, so a developer
	-- can point Capture One at a local build; it is an error, not a fallback,
	-- when it is set to something unusable, because silently running a
	-- different binary than the one asked for is worse than refusing.
	set end of programLines to "lrg_bin=\"\""
	set end of programLines to "if [ -n \"${LRGENIUS_C1_BIN:-}\" ]; then"
	set end of programLines to "  if [ -f \"$LRGENIUS_C1_BIN\" ] && [ -x \"$LRGENIUS_C1_BIN\" ]; then"
	set end of programLines to "    lrg_bin=\"$LRGENIUS_C1_BIN\""
	set end of programLines to "  else"
	set end of programLines to "    echo \"LRGENIUS_C1_BIN is set to:\" >&2"
	set end of programLines to "    echo \"$LRGENIUS_C1_BIN\" >&2"
	set end of programLines to "    echo \"There is no executable file at that path.\" >&2"
	set end of programLines to "    exit 65"
	set end of programLines to "  fi"
	set end of programLines to "fi"
	set end of programLines to "if [ -z \"$lrg_bin\" ]; then"
	set end of programLines to "  for lrg_candidate in \\"
	set end of programLines to "    \"/Applications/LrGeniusAI/Server/lrgenius-c1\" \\"
	set end of programLines to "    \"$HOME/Applications/LrGeniusAI/lrgenius-c1\" \\"
	set end of programLines to "    \"/usr/local/bin/lrgenius-c1\" \\"
	set end of programLines to "    \"/opt/homebrew/bin/lrgenius-c1\""
	set end of programLines to "  do"
	set end of programLines to "    if [ -f \"$lrg_candidate\" ] && [ -x \"$lrg_candidate\" ]; then"
	set end of programLines to "      lrg_bin=\"$lrg_candidate\""
	set end of programLines to "      break"
	set end of programLines to "    fi"
	set end of programLines to "  done"
	set end of programLines to "fi"
	set end of programLines to "if [ -z \"$lrg_bin\" ]; then"
	set end of programLines to "  echo \"No lrgenius-c1 on any known path.\" >&2"
	set end of programLines to "  exit 66"
	set end of programLines to "fi"

	-- Start it, detached, with a dated header in the log so an old crash is
	-- not mistaken for the current one.
	set end of programLines to "{"
	set end of programLines to "  echo \"\""
	set end of programLines to "  echo \"=== $(date '+%Y-%m-%d %H:%M:%S') launching: $lrg_bin" & argumentText & "\""
	set end of programLines to "} >>\"$lrg_log\" 2>/dev/null || true"
	set end of programLines to "nohup \"$lrg_bin\"" & argumentText & " >>\"$lrg_log\" 2>&1 </dev/null &"
	set end of programLines to "lrg_pid=$!"

	-- Backgrounding always \"succeeds\", so the exit status of the launch says
	-- nothing on its own. Wait a moment and look again: a companion that was
	-- killed by Gatekeeper, is missing a library or choked on its arguments is
	-- already gone by now, and that is exactly the failure a user would
	-- otherwise experience as the menu item doing nothing at all.
	set end of programLines to "sleep 0.4"

	-- A process that exited but has not been reaped yet still answers to
	-- kill -0, so ask ps for the state instead: empty means gone, Z means
	-- zombie, anything else means it is running.
	set end of programLines to "lrg_state=\"$(ps -o state= -p \"$lrg_pid\" 2>/dev/null | tr -d '[:space:]')\""
	set end of programLines to "case \"$lrg_state\" in"
	set end of programLines to "  \"\" | Z*) ;;"
	set end of programLines to "  *) exit 0 ;;"
	set end of programLines to "esac"

	-- It is gone. Collect its exit status; 0 just means it was a short job
	-- that finished before we looked, which is a perfectly good outcome.
	set end of programLines to "wait \"$lrg_pid\" 2>/dev/null"
	set end of programLines to "lrg_rc=$?"
	set end of programLines to "if [ \"$lrg_rc\" -eq 0 ]; then"
	set end of programLines to "  exit 0"
	set end of programLines to "fi"
	set end of programLines to "{"
	set end of programLines to "  echo \"The companion exited immediately with status $lrg_rc.\""
	set end of programLines to "  echo \"\""
	set end of programLines to "  echo \"Program: $lrg_bin\""
	set end of programLines to "  echo \"Log: $lrg_log\""
	set end of programLines to "  echo \"\""
	set end of programLines to "  echo \"Last lines of the log:\""
	set end of programLines to "  tail -n 10 \"$lrg_log\" 2>/dev/null || true"
	set end of programLines to "} >&2"
	set end of programLines to "exit 70"

	set programText to ""
	repeat with aLine in programLines
		set programText to programText & (aLine as text) & linefeed
	end repeat
	return programText
end shellProgramFor


-- Turns an exit status into something the user can act on. Every branch ends
-- in a dialog: a menu item that quietly does nothing is the one outcome this
-- whole handler exists to prevent.
on reportProblem(errorNumber, errorText, actionTitle)
	if errorNumber is 66 then
		set messageText to "LrGeniusAI is not installed, so \"" & actionTitle & "\" cannot run." & return & return & ¬
			"Install LrGeniusAI and choose Scripts > Update Scripts Menu in Capture One, then try again." & return & return & ¬
			"The companion program lrgenius-c1 was looked for at:" & return & ¬
			"    /Applications/LrGeniusAI/Server/lrgenius-c1" & return & ¬
			"    ~/Applications/LrGeniusAI/lrgenius-c1" & return & ¬
			"    /usr/local/bin/lrgenius-c1" & return & ¬
			"    /opt/homebrew/bin/lrgenius-c1" & return & return & ¬
			"If it is installed elsewhere, quit Capture One and start it from Terminal with LRGENIUS_C1_BIN set to the full path of lrgenius-c1."
		showMessage(messageText, false)

	else if errorNumber is 65 then
		set messageText to "LrGeniusAI cannot run \"" & actionTitle & "\": the LRGENIUS_C1_BIN override does not point at a program." & return & return & ¬
			errorText & return & return & ¬
			"Correct LRGENIUS_C1_BIN and restart Capture One, or unset it to use the installed copy of lrgenius-c1."
		showMessage(messageText, false)

	else if errorNumber is 70 then
		set messageText to "LrGeniusAI started and stopped again right away, so \"" & actionTitle & "\" did not run." & return & return & ¬
			errorText & return & return & ¬
			"Try running the same command in Terminal to see the full error, then report it with the log if it is not obvious."
		showMessage(messageText, true)

	else
		set messageText to "LrGeniusAI could not start \"" & actionTitle & "\"." & return & return & ¬
			errorText & " (error " & (errorNumber as text) & ")" & return & return & ¬
			"Check that LrGeniusAI is installed, then try again."
		showMessage(messageText, false)
	end if
end reportProblem


-- Shows the message. Falls back to System Events if the host application
-- refuses the dialog, so the report cannot end up nowhere.
on showMessage(messageText, offerLog)
	if offerLog then
		set buttonList to {"Reveal Log", "OK"}
	else
		set buttonList to {"OK"}
	end if
	set chosenButton to "OK"
	try
		set chosenButton to button returned of (display dialog messageText with title "LrGeniusAI" buttons buttonList default button "OK" with icon caution)
	on error
		try
			tell application "System Events"
				activate
				set chosenButton to button returned of (display dialog messageText with title "LrGeniusAI" buttons buttonList default button "OK" with icon caution)
			end tell
		on error
			return
		end try
	end try
	if chosenButton is "Reveal Log" then revealLog()
end showMessage


-- Opens the log in the Finder, or its folder when the file is not there yet.
on revealLog()
	try
		do shell script "lrg_log=\"$HOME/Library/Logs/LrGeniusAI/lrgenius-c1-launch.log\"; if [ -f \"$lrg_log\" ]; then open -R \"$lrg_log\"; else open \"$HOME/Library/Logs\"; fi"
	on error errorText
		showMessage("LrGeniusAI could not open the log folder." & return & return & errorText & return & return & ¬
			"Open it yourself at ~/Library/Logs/LrGeniusAI.", false)
	end try
end revealLog
