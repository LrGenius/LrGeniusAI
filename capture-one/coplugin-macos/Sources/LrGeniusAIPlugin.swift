//
//  LrGeniusAIPlugin.swift
//
//  Capture One "Open With" plugin that hands the *original* files of the current
//  selection to the LrGeniusAI backend (127.0.0.1:19819), falling back to launching
//  the `lrgenius-c1` helper when the backend is not answering.
//
//  Scope, deliberately: this is a shim. It collects paths and hands them off. It does
//  not read metadata, does not write keywords, does not talk to the catalog — the
//  Capture One plugin API exposes none of that (see ../README.md). Anything richer
//  runs through the AppleScript entry point in ../scripts-macos/.
//
//  Statelessness is a hard requirement, not a style choice: Capture One hosts plugins
//  out-of-process in COPluginHostApple.xpc, calls these methods in arbitrary order,
//  and may relaunch the host between calls. Every method below is a pure function of
//  its arguments plus the on-disk environment. There are no stored properties.
//

import AppKit
import CaptureOnePlugins
import Foundation
import os.log

// MARK: - Configuration

private enum Config {
    /// Reverse-DNS prefix shared by the action and every settings identifier, matching
    /// the convention the shipping third-party plugins use.
    static let bundleID = "cloud.machek.lrgeniusai.coplugin"

    static let openWithActionID = bundleID + ".openwith"
    static let settingsGroupID = bundleID + ".group"
    static let settingsInfoID = bundleID + ".info"
    static let settingsOpenButtonID = bundleID + ".openapp"
    static let settingsStatusID = bundleID + ".status"

    /// Echoed back to us through -handleEvent:forSettingsItem:error:callback: so the
    /// button identifies itself without the plugin holding any state.
    static let openButtonContext = bundleID + ".openapp.context"

    static let backendHost = "127.0.0.1"
    static let backendPort = 19819

    static var backendBase: URL {
        // Fixed host/port, so this cannot fail; a force-unwrap here is honest.
        URL(string: "http://\(backendHost):\(backendPort)")!
    }

    /// Liveness probe. Plain-text "pong".
    static var pingURL: URL { backendBase.appendingPathComponent("ping") }

    /// NOTE: this endpoint does NOT exist in the backend yet. `server-rs` currently has
    /// no /v1/host/handoff route — adding it is a separate, deliberate piece of work and
    /// is explicitly out of scope for this change (no backend edits). Until it lands the
    /// POST below will return 404, which this plugin treats exactly like "backend not
    /// usable" and resolves through the `lrgenius-c1` fallback path. That is why the
    /// fallback is not an afterthought here: today it is the *only* working path.
    static var handoffURL: URL { backendBase.appendingPathComponent("v1/host/handoff") }

    static let pingTimeout: TimeInterval = 1.5
    static let handoffTimeout: TimeInterval = 30.0

    /// Environment override for the helper binary, mostly for development.
    static let helperEnvVar = "LRGENIUS_C1"

    static let helperCandidates = [
        "/Applications/LrGeniusAI/Server/lrgenius-c1",
        "/Applications/LrGeniusAI/lrgenius-c1",
        "/usr/local/bin/lrgenius-c1",
        "/opt/homebrew/bin/lrgenius-c1",
        NSHomeDirectory() + "/.local/bin/lrgenius-c1",
    ]

    static let log = OSLog(subsystem: bundleID, category: "plugin")
}

// MARK: - Errors that reach the user

/// Capture One surfaces a thrown NSError's `localizedDescription` to the user, and that
/// string is the plugin's ONLY user-visible channel — there is no warning channel and no
/// window of our own. So every case here says what to DO, not what happened internally.
private enum HandoffError: LocalizedError {
    case noFiles
    case noReadableFiles(tried: Int, reasons: [String])
    case cancelled
    case backendRejected(status: Int, body: String)
    case helperMissing(underlying: String)
    case helperFailed(status: Int32, output: String)

    var errorDescription: String? {
        switch self {
        case .noFiles:
            return "Capture One passed no files to LrGeniusAI. Select one or more images in the Browser, then try again."

        case let .noReadableFiles(tried, reasons):
            let detail = Summary.render(reasons, limit: 3)
            return """
            None of the \(tried) selected file\(tried == 1 ? "" : "s") could be read from disk. \
            Reconnect the drive holding these images, or locate them in Capture One \
            (Image menu > Locate), then try again.
            \(detail)
            """

        case .cancelled:
            return "The LrGeniusAI hand-off was cancelled."

        case let .backendRejected(status, body):
            if status == 404 {
                return """
                This copy of the LrGeniusAI backend does not support hand-off from Capture One yet. \
                Update LrGeniusAI to a build that provides /v1/host/handoff, or install the \
                lrgenius-c1 helper so the plugin can fall back to it.
                """
            }
            let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines).prefix(300)
            return """
            The LrGeniusAI backend refused the hand-off (HTTP \(status)). \
            Open the LrGeniusAI app and check its log, then try again.
            \(trimmed.isEmpty ? "" : "Backend said: \(trimmed)")
            """

        case let .helperMissing(underlying):
            return """
            LrGeniusAI is not running and the lrgenius-c1 helper could not be found. \
            Start the LrGeniusAI app once so its backend is listening on port \(Config.backendPort), \
            or install lrgenius-c1 into /usr/local/bin.
            (\(underlying))
            """

        case let .helperFailed(status, output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines).prefix(300)
            return """
            The lrgenius-c1 helper exited with code \(status). \
            Run it once from Terminal to see the full message, then try again.
            \(trimmed.isEmpty ? "" : "It printed: \(trimmed)")
            """
        }
    }
}

// MARK: - Warning summarisation

private enum Summary {
    /// Deduplicates preserving order, shows a handful, counts the rest. A run-wide cause
    /// (a whole volume offline) collapses to one line instead of one line per photo.
    static func render(_ messages: [String], limit: Int) -> String {
        var seen = Set<String>()
        var unique: [String] = []
        for m in messages where !seen.contains(m) {
            seen.insert(m)
            unique.append(m)
        }
        guard !unique.isEmpty else { return "" }
        let shown = unique.prefix(limit).map { "- \($0)" }.joined(separator: "\n")
        let rest = unique.count - min(limit, unique.count)
        return rest > 0 ? "\(shown)\n- ... and \(rest) more" : shown
    }
}

// MARK: - Plugin

@objc(LrGeniusAIPlugin)
public final class LrGeniusAIPlugin: COPluginBase, COOpenWithPlugin, COSettings {

    // MARK: COOpenWithPlugin — action enumeration
    //
    // This runs synchronously while Capture One is building the context menu. It must
    // not touch the network: a 1.5 s ping against a dead port would stall the menu for
    // every right-click. Whether the backend is up is decided later, inside the task.

    public func openWithActions(
        withFileInfo fileInfo: [AnyHashable: Any]?,
        pluginRole: COPluginRole
    ) throws -> [COPluginAction] {
        // Only the Open With role. Publish and colour-profiling reuse this callback and
        // expect COPluginActionPublishResult / ...ColorProfilingResult respectively, which
        // this plugin does not produce.
        guard pluginRole == .openWith else { return [] }

        // fileInfo maps a file-kind key to an NSNumber count. Verified from the shipping
        // Helicon plugin, which gates on [[fileInfo allValues] valueForKeyPath:@"@sum.intValue"].
        // A nil fileInfo means "no filtering information", so we stay available.
        if let info = fileInfo, !info.isEmpty {
            let total = (info.values.compactMap { ($0 as? NSNumber)?.intValue }).reduce(0, +)
            guard total > 0 else { return [] }
        }

        let action = COPluginAction(displayName: "Send to LrGeniusAI")
        action.identifier = Config.openWithActionID
        action.image = Self.menuIcon()
        return [action]
    }

    // MARK: COFileHandling — one task for the whole selection
    //
    // Returning one task per file would produce one hand-off per image. The backend wants
    // the selection as a batch, so this deliberately returns exactly one task.

    public func tasks(for action: COPluginAction, forFiles files: [String]) throws -> [COPluginTask] {
        guard !files.isEmpty else { throw HandoffError.noFiles }
        return [COFileHandlingPluginTask(action: action, files: files)]
    }

    // MARK: COOpenWithPlugin — the actual work

    public func startOpen(
        with task: COFileHandlingPluginTask,
        progress: COPluginTaskProgress?
    ) throws -> COPluginActionOpenWithResult {
        let files = task.files
        guard !files.isEmpty else { throw HandoffError.noFiles }

        // +1 unit for the hand-off itself, after the per-file validation units.
        let total = UInt(files.count + 1)
        var completed: UInt = 0
        progress?(task, completed, total, "Checking files...")

        var usable: [String] = []
        var problems: [String] = []

        for path in files {
            if task.cancelled { throw HandoffError.cancelled }

            var isDirectory: ObjCBool = false
            let exists = FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)
            if !exists {
                problems.append("missing from disk: \((path as NSString).lastPathComponent)")
            } else if isDirectory.boolValue {
                problems.append("is a folder, not an image: \((path as NSString).lastPathComponent)")
            } else if !FileManager.default.isReadableFile(atPath: path) {
                problems.append("not readable (check permissions): \((path as NSString).lastPathComponent)")
            } else {
                usable.append(path)
            }

            completed += 1
            progress?(task, completed, total, "Checking files...")
        }

        guard !usable.isEmpty else {
            throw HandoffError.noReadableFiles(tried: files.count, reasons: problems)
        }

        if task.cancelled { throw HandoffError.cancelled }

        let documentType = Self.documentType(from: task.environment)

        // A degraded success still has to reach the user. The plugin API gives us no
        // warning channel of its own, so the skipped files travel *in the payload*:
        // LrGeniusAI is responsible for showing them, which keeps the report chain intact
        // rather than ending it in a log line here.
        let skipped = Array(Set(problems)).sorted()

        progress?(task, completed, total, "Handing \(usable.count) file\(usable.count == 1 ? "" : "s") to LrGeniusAI...")

        do {
            try Self.handOff(
                paths: usable,
                documentType: documentType,
                skipped: skipped,
                temporaryFolder: Self.temporaryFolder(from: task.environment),
                isCancelled: { task.cancelled }
            )
        } catch let error as HandoffError {
            os_log("hand-off failed: %{public}@", log: Config.log, type: .error, error.errorDescription ?? "unknown")
            throw error
        }

        progress?(task, total, total, "Handed off to LrGeniusAI.")

        let result = COPluginActionOpenWithResult(status: true)
        // We open our own UI; Capture One's "done" notification would be noise on top.
        result.suppressNotification = true
        return result
    }

    // MARK: COSettings — a minimal, declarative form
    //
    // The settings tree is pure data; there is no view code and no way to draw a custom
    // window. One informative label plus one button is the whole surface.

    public func settings() throws -> [COSettingsElement] {
        let info = COSettingsLabelItem(identifier: Config.settingsInfoID, title: "About")
        info.value = """
        Select images in Capture One and choose "Send to LrGeniusAI" from the \
        Open With menu. LrGeniusAI reads the original files directly — Capture One \
        does not need to process or export them first.
        """

        let status = COSettingsLabelItem(identifier: Config.settingsStatusID, title: "Backend")
        status.value = "Expected at http://\(Config.backendHost):\(Config.backendPort)"
        status.informativeText = "Start the LrGeniusAI app if hand-off reports that the backend is unavailable."

        let button = COSettingsButtonItem(identifier: Config.settingsOpenButtonID, title: "Open LrGeniusAI")
        button.context = Config.openButtonContext as NSString
        button.informativeText = "Opens the LrGeniusAI interface in your browser."

        let items = COSettingsItemsGroup(identifier: Config.settingsGroupID + ".items", title: nil)
        items.items = [info, status, button]

        let group = COSettingsElementsGroup(identifier: Config.settingsGroupID, title: "LrGeniusAI")
        group.elements = [items]

        return [group]
    }

    public func didUpdateValue(
        _ value: (any NSSecureCoding)?,
        forSetting settingIdentifier: String,
        callback: ((COSettingsCallbackAction, (any NSCopying & NSSecureCoding)?) -> Void)?
    ) throws {
        // Nothing in this form is editable, so there is no value to persist. Declared
        // because COSettings requires it.
        os_log("ignoring value change for %{public}@", log: Config.log, type: .debug, settingIdentifier)
    }

    public func handleEvent(
        _ event: COSettingsEvent,
        for item: COSettingsItem,
        callback: ((COSettingsCallbackAction, (any NSCopying & NSSecureCoding)?) -> Void)?
    ) throws {
        // Identify the button by identifier, and by the context it carries, rather than
        // by the opaque event code, whose values are not documented.
        let contextMatches = (item as? COSettingsButtonItem)
            .flatMap { $0.context as? NSString }
            .map { $0 as String } == Config.openButtonContext

        guard item.identifier == Config.settingsOpenButtonID || contextMatches else { return }

        if Self.backendIsUp() {
            NSWorkspace.shared.open(Config.backendBase)
            return
        }

        // Backend down: start the helper with no files, which brings the app up.
        do {
            _ = try Self.runHelper(arguments: ["open"])
        } catch let error as HandoffError {
            throw error
        }
    }

    // MARK: - Hand-off

    private static func handOff(
        paths: [String],
        documentType: String,
        skipped: [String],
        temporaryFolder: String,
        isCancelled: () -> Bool
    ) throws {
        var payload: [String: Any] = [
            "host": "captureone",
            "document_type": documentType,
            "paths": paths,
        ]
        if !skipped.isEmpty {
            payload["skipped"] = skipped
        }

        // Try HTTP first, but only if something is actually listening — a short ping keeps
        // the failure fast when LrGeniusAI simply is not running.
        if backendIsUp() {
            let body = try JSONSerialization.data(withJSONObject: payload, options: [])
            var request = URLRequest(url: Config.handoffURL)
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
            request.timeoutInterval = Config.handoffTimeout

            let (status, data) = try send(request, timeout: Config.handoffTimeout, isCancelled: isCancelled)

            if (200..<300).contains(status) {
                return
            }

            // 404 means this backend predates /v1/host/handoff. Everything else is a real
            // refusal and should be reported rather than silently retried through the helper.
            guard status == 404 else {
                throw HandoffError.backendRejected(status: status, body: String(decoding: data, as: UTF8.self))
            }
            os_log("backend has no /v1/host/handoff (404); using lrgenius-c1", log: Config.log, type: .info)
        }

        if isCancelled() { throw HandoffError.cancelled }

        // Fallback. Paths go through a list file rather than argv: a Capture One selection
        // can run to thousands of images and blow past ARG_MAX. (The shipping Helicon plugin
        // uses the same list-file technique.)
        let listPath = (temporaryFolder as NSString)
            .appendingPathComponent("lrgeniusai-handoff-\(UUID().uuidString).txt")
        let listBody = paths.joined(separator: "\n") + "\n"
        do {
            try listBody.write(toFile: listPath, atomically: true, encoding: .utf8)
        } catch {
            throw HandoffError.helperMissing(underlying: "could not write the file list to \(temporaryFolder): \(error.localizedDescription)")
        }
        defer { try? FileManager.default.removeItem(atPath: listPath) }

        var arguments = [
            "handoff",
            "--host", "captureone",
            "--document-type", documentType,
            "--paths-from", listPath,
        ]
        if !skipped.isEmpty {
            arguments.append(contentsOf: ["--skipped-count", String(skipped.count)])
        }

        _ = try runHelper(arguments: arguments)
    }

    /// Synchronous POST that stays responsive to Capture One's cancel button.
    ///
    /// `startOpenWithTask:error:progress:` is a synchronous call on a host-owned worker
    /// thread, so blocking here is expected — but blocking on a bare semaphore would make
    /// Cancel do nothing for the whole timeout. Polling in short slices fixes that.
    private static func send(
        _ request: URLRequest,
        timeout: TimeInterval,
        isCancelled: () -> Bool
    ) throws -> (Int, Data) {
        let semaphore = DispatchSemaphore(value: 0)
        var responseStatus = 0
        var responseData = Data()
        var transportError: Error?

        let dataTask = URLSession.shared.dataTask(with: request) { data, response, error in
            if let http = response as? HTTPURLResponse { responseStatus = http.statusCode }
            if let data { responseData = data }
            transportError = error
            semaphore.signal()
        }
        dataTask.resume()

        let deadline = Date().addingTimeInterval(timeout)
        while true {
            if semaphore.wait(timeout: .now() + 0.1) == .success { break }
            if isCancelled() {
                dataTask.cancel()
                throw HandoffError.cancelled
            }
            if Date() >= deadline {
                dataTask.cancel()
                transportError = URLError(.timedOut)
                break
            }
        }

        if let transportError {
            // Reaching the backend failed outright. Report it as "not usable" so the caller
            // moves on to the helper instead of surfacing a URLError the user cannot act on.
            os_log("transport error: %{public}@", log: Config.log, type: .info, transportError.localizedDescription)
            return (0, Data())
        }
        return (responseStatus, responseData)
    }

    private static func backendIsUp() -> Bool {
        var request = URLRequest(url: Config.pingURL)
        request.httpMethod = "GET"
        request.timeoutInterval = Config.pingTimeout
        let (status, body) = (try? send(request, timeout: Config.pingTimeout, isCancelled: { false })) ?? (0, Data())
        guard (200..<300).contains(status) else { return false }
        return String(decoding: body, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .hasPrefix("pong")
    }

    // MARK: - Helper process

    @discardableResult
    private static func runHelper(arguments: [String]) throws -> String {
        guard let helper = helperPath() else {
            throw HandoffError.helperMissing(
                underlying: "looked in " + Config.helperCandidates.joined(separator: ", ")
            )
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: helper)
        process.arguments = arguments

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        do {
            try process.run()
        } catch {
            throw HandoffError.helperMissing(underlying: "\(helper) could not be started: \(error.localizedDescription)")
        }

        let output = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        process.waitUntilExit()

        guard process.terminationStatus == 0 else {
            throw HandoffError.helperFailed(status: process.terminationStatus, output: output)
        }
        return output
    }

    private static func helperPath() -> String? {
        if let override = ProcessInfo.processInfo.environment[Config.helperEnvVar],
           FileManager.default.isExecutableFile(atPath: override) {
            return override
        }
        return Config.helperCandidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    // MARK: - Environment helpers

    private static func documentType(from environment: [AnyHashable: Any]?) -> String {
        guard let raw = environment?[COPluginTaskExecutingDocumentType] as? String else {
            return "unknown"
        }
        // Normalise the SDK's opaque constants to the plain words the backend expects.
        switch raw {
        case COPluginTaskDocumentTypeCatalog: return "catalog"
        case COPluginTaskDocumentTypeSession: return "session"
        default: return raw
        }
    }

    private static func temporaryFolder(from environment: [AnyHashable: Any]?) -> String {
        if let folder = environment?[COPluginTaskTemporaryFolder] as? String, !folder.isEmpty {
            return folder
        }
        return NSTemporaryDirectory()
    }

    private static func menuIcon() -> NSImage? {
        // Bundle(for:) resolves inside the XPC host, where the plugin bundle is loaded.
        guard let url = Bundle(for: LrGeniusAIPlugin.self).url(forResource: "LrGeniusAI", withExtension: "icns") else {
            return nil
        }
        return NSImage(contentsOf: url)
    }
}
