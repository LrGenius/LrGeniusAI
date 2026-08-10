// Entry point: read JSON-lines requests from stdin, serve them in order, write
// JSON-lines responses to stdout.
//
// Nothing here may write to stdout except `respond` — stdout is the protocol
// channel and a stray `print` corrupts it. Diagnostics go to stderr via `Log`,
// which the Rust side relays into the server log.

import Foundation

/// stderr logging. Prefixed so the Rust supervisor can classify a line without
/// parsing it, and unbuffered so a crash does not swallow the last thing the
/// process was doing.
enum Log {
    static func info(_ message: String) { emit("INFO", message) }
    static func error(_ message: String) { emit("ERROR", message) }

    private static func emit(_ level: String, _ message: String) {
        FileHandle.standardError.write(Data("[\(level)] \(message)\n".utf8))
    }
}

private let encoder = JSONEncoder()
private let decoder = JSONDecoder()

private func respond(_ response: SidecarResponse) {
    guard var line = try? encoder.encode(response) else {
        // Encoding our own response type cannot realistically fail, but if it
        // did, silence would hang the server waiting on a reply it will never
        // get. Emit a hand-built envelope instead.
        let fallback = #"{"id":\#(response.id),"ok":false,"error":"response encoding failed"}"#
        FileHandle.standardOutput.write(Data((fallback + "\n").utf8))
        return
    }
    line.append(0x0A)  // newline; the payload itself never contains a raw one
    FileHandle.standardOutput.write(line)
}

private let engine = Engine()

private func handle(_ request: SidecarRequest) async {
    switch request.op {
    case .ping:
        respond(.ok(id: request.id))

    case .load:
        guard let modelDir = request.modelDir else {
            respond(.failure(id: request.id, "load requires model_dir"))
            return
        }
        do {
            let info = try await engine.load(modelDir: modelDir)
            respond(.loaded(id: request.id, info: info))
        } catch {
            Log.error("load failed: \(describe(error))")
            respond(.failure(id: request.id, describe(error)))
        }

    case .generate:
        guard let specs = request.requests else {
            respond(.failure(id: request.id, "generate requires requests"))
            return
        }
        do {
            let results = try await engine.generate(specs: specs)
            respond(.generated(id: request.id, results: results))
        } catch {
            // Only whole-batch failures land here (no model loaded, say);
            // per-photo failures are inside `results`.
            Log.error("generate failed: \(describe(error))")
            respond(.failure(id: request.id, describe(error)))
        }

    case .status:
        respond(.status(id: request.id, info: engine.info()))

    case .unload:
        engine.unload()
        respond(.ok(id: request.id))

    case .shutdown:
        respond(.ok(id: request.id))
        engine.unload()
        exit(0)
    }
}

/// Read stdin line by line.
///
/// `FileHandle.bytes.lines` is not used: it is available only from macOS 12 in
/// a form that also buffers aggressively, and a batch request carrying several
/// photo prompts can exceed the sizes it handles comfortably. Reading raw and
/// splitting on newlines keeps the framing explicit.
private func readLines(_ handle: FileHandle, _ body: (Data) async -> Void) async {
    var buffer = Data()
    while true {
        let chunk = handle.availableData
        if chunk.isEmpty {
            // EOF: the server closed the pipe or went away. Anything still in
            // the buffer is a truncated line and is deliberately dropped.
            break
        }
        buffer.append(chunk)
        while let newline = buffer.firstIndex(of: 0x0A) {
            let line = buffer[buffer.startIndex ..< newline]
            buffer.removeSubrange(buffer.startIndex ... newline)
            if !line.isEmpty {
                await body(Data(line))
            }
        }
    }
}

// MLX's first Metal allocation is slow and the Rust side has a load timeout, so
// announce readiness before touching anything heavy.
Log.info("lrgenius-mlx sidecar started")

await readLines(FileHandle.standardInput) { line in
    guard let request = try? decoder.decode(SidecarRequest.self, from: line) else {
        // No id to echo, so this cannot be tied to a caller's pending request.
        // The supervisor times that one out; logging is all we can usefully do.
        Log.error("could not decode request: \(String(decoding: line, as: UTF8.self).prefix(200))")
        return
    }
    await handle(request)
}

Log.info("stdin closed, exiting")
