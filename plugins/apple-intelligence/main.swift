// rush plugin: Apple Intelligence's on-device model for rush's small jobs.
// JSON-RPC 2.0 on fd 3, each message behind a 4-byte big-endian length.
import Foundation
import FoundationModels

let wlock = NSLock()

func writeAll(_ data: Data) {
    wlock.lock()
    defer { wlock.unlock() }
    data.withUnsafeBytes { (p: UnsafeRawBufferPointer) in
        var off = 0
        while off < p.count {
            let n = write(3, p.baseAddress! + off, p.count - off)
            if n <= 0 { if n < 0 && errno == EINTR { continue }; return }
            off += n
        }
    }
}

func send(_ obj: [String: Any]) {
    var o = obj
    o["jsonrpc"] = "2.0"
    guard let body = try? JSONSerialization.data(withJSONObject: o) else { return }
    var len = UInt32(body.count).bigEndian
    var out = Data(bytes: &len, count: 4)
    out.append(body)
    writeAll(out)
}

func readExact(_ n: Int) -> Data? {
    var buf = [UInt8](repeating: 0, count: n)
    var off = 0
    while off < n {
        let r = buf.withUnsafeMutableBytes { read(3, $0.baseAddress! + off, n - off) }
        if r == 0 { return nil }
        if r < 0 { if errno == EINTR { continue }; return nil }
        off += r
    }
    return Data(buf)
}

struct RPCError: Error {
    let code: Int
    let message: String
}

let reserve = 512

func availability() -> (Bool, String) {
    switch SystemLanguageModel.default.availability {
    case .available:
        return (true, "")
    case .unavailable(let reason):
        switch reason {
        case .appleIntelligenceNotEnabled:
            return (false, "Apple Intelligence is off: turn it on in System Settings › Apple Intelligence & Siri")
        case .deviceNotEligible:
            return (false, "this Mac can't run it")
        case .modelNotReady:
            return (false, "the model is still downloading")
        @unknown default:
            return (false, "the on-device model is unavailable")
        }
    }
}

func modelsList() -> [String: Any] {
    let (ok, why) = availability()
    return ["models": [[
        "name": "on-device",
        "label": "Apple on-device model",
        "contextTokens": SystemLanguageModel.default.contextSize,
        "available": ok,
        "why": why,
    ] as [String: Any]]]
}

func tooSmall(_ need: Int?) -> RPCError {
    let size = SystemLanguageModel.default.contextSize
    let needs = need.map { "this needs \($0)" } ?? "this needs more"
    return RPCError(code: -32000, message: "too small: the on-device model holds \(size) tokens, \(needs)")
}

func modelsAsk(_ params: [String: Any]) async throws -> [String: Any] {
    let name = params["model"] as? String ?? "on-device"
    guard name == "on-device" else { throw RPCError(code: -32602, message: "no model named \(name)") }
    guard let prompt = params["prompt"] as? String else { throw RPCError(code: -32602, message: "prompt is required") }
    let system = params["system"] as? String ?? ""
    let (ok, why) = availability()
    guard ok else { throw RPCError(code: -32000, message: "the on-device model is unavailable: \(why)") }

    let model = SystemLanguageModel.default
    let size = model.contextSize
    if #available(macOS 26.4, *) {
        if let used = try? await model.tokenCount(for: system + "\n" + prompt), used + reserve > size {
            throw tooSmall(used + reserve)
        }
    }
    let session = system.isEmpty
        ? LanguageModelSession(model: model)
        : LanguageModelSession(model: model, instructions: system)
    do {
        let r = try await session.respond(to: prompt)
        return ["text": r.content]
    } catch let e as LanguageModelSession.GenerationError {
        switch e {
        case .exceededContextWindowSize: throw tooSmall(nil)
        case .guardrailViolation, .refusal: throw RPCError(code: -32000, message: "the on-device model declined to answer this")
        default: throw RPCError(code: -32000, message: "the on-device model failed: \(e.localizedDescription)")
        }
    } catch {
        throw RPCError(code: -32000, message: "the on-device model failed: \(error.localizedDescription)")
    }
}

func handle(_ method: String, _ params: [String: Any]) async throws -> [String: Any] {
    switch method {
    case "initialize": return [:]
    case "models.list": return modelsList()
    case "models.ask": return try await modelsAsk(params)
    default: throw RPCError(code: -32601, message: "method not found: \(method)")
    }
}

// Reader runs on the main thread; each request gets its own Task.
while let hdr = readExact(4) {
    let n = Int(hdr.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) })
    guard let body = readExact(n) else { break }
    guard let obj = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { continue }
    guard let method = obj["method"] as? String else { continue } // a reply: we make no calls
    let params = obj["params"] as? [String: Any] ?? [:]
    guard let id = obj["id"] else { // notification
        continue
    }
    Task.detached {
        do {
            let res = try await handle(method, params)
            send(["id": id, "result": res])
        } catch let e as RPCError {
            send(["id": id, "error": ["code": e.code, "message": e.message]])
        } catch {
            send(["id": id, "error": ["code": -32000, "message": "\(error)"]])
        }
    }
}
