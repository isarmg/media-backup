import Foundation

@_silgen_name("pb_open") private func pb_open(_ databasePath: UnsafePointer<CChar>, _ configJSON: UnsafePointer<CChar>) -> UInt64
@_silgen_name("pb_close") private func pb_close(_ handle: UInt64)
@_silgen_name("pb_needs") private func pb_needs(_ handle: UInt64, _ asset: UnsafePointer<CChar>, _ resource: UnsafePointer<CChar>, _ modified: Int64) -> Bool
@_silgen_name("pb_enqueue") private func pb_enqueue(_ handle: UInt64, _ input: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_next") private func pb_next(_ handle: UInt64, _ staging: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_mark_upload") private func pb_mark_upload(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ upload: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_mark_part") private func pb_mark_part(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ index: UInt32) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_mark_complete") private func pb_mark_complete(_ handle: UInt64, _ job: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_mark_failed") private func pb_mark_failed(_ handle: UInt64, _ job: UnsafePointer<CChar>, _ error: UnsafePointer<CChar>, _ retryable: Bool) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_stats") private func pb_stats(_ handle: UInt64) -> UnsafeMutablePointer<CChar>?
@_silgen_name("pb_last_error") private func pb_last_error() -> UnsafePointer<CChar>?
@_silgen_name("pb_string_free") private func pb_string_free(_ value: UnsafeMutablePointer<CChar>?)

struct UploadPart: Codable {
    let index: UInt32
    let ciphertextSize: UInt64
    let ciphertextBlake3: String
}

struct CreateUpload: Codable {
    let sourceAssetId: String
    let sourceResourceId: String
    let mediaKind: String
    let role: String
    let filename: String
    let mimeType: String
    let sourceCreatedAtMs: Int64
    let plaintextSize: UInt64
    let dedupToken: String
    let wrappedKey: String
    let keyNonce: String
    let noncePrefix: String
    let metadataNonce: String?
    let metadataCiphertext: String?
    let parts: [UploadPart]
}

struct LocalPart: Codable {
    let index: UInt32
    let path: String
}

struct PreparedJob: Codable {
    let jobId: String
    let request: CreateUpload
    let localParts: [LocalPart]
}

struct EnqueueInput: Codable {
    let sourceAssetId: String
    let sourceResourceId: String
    let mediaKind: String
    let role: String
    let filePath: String
    let filename: String
    let mimeType: String
    let sourceCreatedAtMs: Int64
    let modifiedMs: Int64
    let sourceSize: UInt64
    let metadataJson: String?
    let removeSourceAfterPrepare: Bool
}

private struct Envelope<T: Decodable>: Decodable {
    let ok: Bool
    let value: T?
    let error: String?
}

private struct EmptyValue: Decodable {}

final class RustAgent {
    private let handle: UInt64
    private let encoder: JSONEncoder = {
        let value = JSONEncoder()
        value.keyEncodingStrategy = .convertToSnakeCase
        return value
    }()
    private let decoder: JSONDecoder = {
        let value = JSONDecoder()
        value.keyDecodingStrategy = .convertFromSnakeCase
        return value
    }()

    init(databasePath: String, masterKey: String, dedupeKey: String) throws {
        let config: [String: Any] = [
            "master_key_b64": masterKey,
            "dedupe_key_b64": dedupeKey,
            "part_size": 16 * 1024 * 1024,
        ]
        let data = try JSONSerialization.data(withJSONObject: config)
        let json = String(decoding: data, as: UTF8.self)
        handle = databasePath.withCString { path in json.withCString { pb_open(path, $0) } }
        if handle == 0 {
            throw AgentFailure.message(pb_last_error().map(String.init(cString:)) ?? "无法打开 Rust Agent")
        }
    }

    deinit { pb_close(handle) }

    func needs(asset: String, resource: String, modifiedMs: Int64) -> Bool {
        asset.withCString { assetPointer in
            resource.withCString { resourcePointer in
                pb_needs(handle, assetPointer, resourcePointer, modifiedMs)
            }
        }
    }

    func enqueue(_ input: EnqueueInput) throws {
        let json = String(decoding: try encoder.encode(input), as: UTF8.self)
        let _: String? = try call { json.withCString { pb_enqueue(handle, $0) } }
    }

    func next(stagingRoot: String) throws -> PreparedJob? {
        try call { stagingRoot.withCString { pb_next(handle, $0) } }
    }

    func markUpload(job: String, upload: String) throws {
        let _: EmptyValue? = try call {
            job.withCString { jobPointer in upload.withCString { pb_mark_upload(handle, jobPointer, $0) } }
        }
    }

    func markPart(job: String, index: UInt32) throws {
        let _: EmptyValue? = try call { job.withCString { pb_mark_part(handle, $0, index) } }
    }

    func markComplete(job: String) throws {
        let _: EmptyValue? = try call { job.withCString { pb_mark_complete(handle, $0) } }
    }

    func markFailed(job: String, error: String) {
        let _: EmptyValue? = try? call {
            job.withCString { jobPointer in
                error.withCString { pb_mark_failed(handle, jobPointer, $0, true) }
            }
        }
    }

    func statsDescription() -> String {
        guard let pointer = pb_stats(handle) else { return "" }
        defer { pb_string_free(pointer) }
        return String(cString: pointer)
    }

    private func call<T: Decodable>(_ operation: () -> UnsafeMutablePointer<CChar>?) throws -> T? {
        guard let pointer = operation() else { throw AgentFailure.message("Rust Agent 返回空值") }
        defer { pb_string_free(pointer) }
        let data = Data(String(cString: pointer).utf8)
        let envelope = try decoder.decode(Envelope<T>.self, from: data)
        if !envelope.ok { throw AgentFailure.message(envelope.error ?? "Rust Agent 操作失败") }
        return envelope.value
    }
}

enum AgentFailure: Error { case message(String) }
