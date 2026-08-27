import Foundation
import Photos

struct RemoteResource: Decodable, Identifiable {
    let resourceId: UUID
    let role: String
    let filename: String
    let mimeType: String
    let contentSize: UInt64
    let storageEncoding: String
    let contentPath: String

    var id: UUID { resourceId }
}

struct RemoteAsset: Decodable, Identifiable {
    let assetId: UUID
    let sourceCreatedAtMs: Int64
    let favorite: Bool
    let archived: Bool
    let trashedAtMs: Int64?
    let tagNames: [String]
    let resources: [RemoteResource]

    var id: UUID { assetId }
    var isTrashed: Bool { trashedAtMs != nil }
    var primary: RemoteResource? {
        resources.first(where: { $0.role == "primary" })
            ?? resources.first(where: { $0.role != "thumbnail" })
    }
    var thumbnail: RemoteResource? { resources.first(where: { $0.role == "thumbnail" }) }
}

private struct TimelinePage: Decodable {
    let items: [RemoteAsset]
    let nextCursor: String?
}

private struct SyncPage: Decodable {
    let nextSequence: Int64
    let hasMore: Bool
}

struct RemoteLibrary {
    let serverURL: URL
    let token: String

    private let decoder: JSONDecoder = {
        let value = JSONDecoder()
        value.keyDecodingStrategy = .convertFromSnakeCase
        return value
    }()

    func timeline(trashed: Bool) async throws -> [RemoteAsset] {
        var result: [RemoteAsset] = []
        var cursor: String?
        repeat {
            var components = URLComponents(url: serverURL.appending(path: "/v1/timeline"), resolvingAgainstBaseURL: false)
            components?.queryItems = [
                URLQueryItem(name: "trashed", value: trashed ? "true" : "false"),
                URLQueryItem(name: "limit", value: "100"),
                cursor.map { URLQueryItem(name: "cursor", value: $0) },
            ].compactMap { $0 }
            guard let url = components?.url else { throw RemoteLibraryError.invalidURL }
            let (data, response) = try await URLSession.shared.data(for: authorized(url: url, method: "GET"))
            try requireSuccess(response, data: data)
            let page = try decoder.decode(TimelinePage.self, from: data)
            result.append(contentsOf: page.items)
            guard page.nextCursor == nil || page.nextCursor != cursor else {
                throw RemoteLibraryError.invalidCursor
            }
            cursor = page.nextCursor
        } while cursor != nil
        return result
    }

    func advanceSync(from initialSequence: Int64) async throws -> Int64 {
        var sequence = initialSequence
        var more: Bool
        repeat {
            var components = URLComponents(url: serverURL.appending(path: "/v1/sync"), resolvingAgainstBaseURL: false)
            components?.queryItems = [
                URLQueryItem(name: "after", value: String(sequence)),
                URLQueryItem(name: "limit", value: "1000"),
            ]
            guard let url = components?.url else { throw RemoteLibraryError.invalidURL }
            let (data, response) = try await URLSession.shared.data(for: authorized(url: url, method: "GET"))
            try requireSuccess(response, data: data)
            let page = try decoder.decode(SyncPage.self, from: data)
            sequence = page.nextSequence
            more = page.hasMore
        } while more
        return sequence
    }

    func setFavorite(asset: RemoteAsset, value: Bool) async throws {
        try await sendJSON(path: "/v1/assets/\(asset.assetId)", method: "PATCH", body: ["favorite": value])
    }

    func setArchived(asset: RemoteAsset, value: Bool) async throws {
        try await sendJSON(path: "/v1/assets/\(asset.assetId)", method: "PATCH", body: ["archived": value])
    }

    func addTag(named rawName: String, to asset: RemoteAsset) async throws {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        let (listData, listResponse) = try await URLSession.shared.data(for: authorized(
            url: serverURL.appending(path: "/v1/tags"),
            method: "GET"
        ))
        try requireSuccess(listResponse, data: listData)
        let tags = try decoder.decode([RemoteTag].self, from: listData)
        let tag: RemoteTag
        if let existing = tags.first(where: { $0.name.caseInsensitiveCompare(name) == .orderedSame }) {
            tag = existing
        } else {
            var request = authorized(url: serverURL.appending(path: "/v1/tags"), method: "POST")
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: ["name": name])
            let (data, response) = try await URLSession.shared.data(for: request)
            try requireSuccess(response, data: data)
            tag = try decoder.decode(RemoteTag.self, from: data)
        }
        try await send(path: "/v1/tags/\(tag.tagId)/assets/\(asset.assetId)", method: "POST")
    }

    func duplicateGroupCount() async throws -> Int {
        let (data, response) = try await URLSession.shared.data(for: authorized(
            url: serverURL.appending(path: "/v1/duplicates"),
            method: "GET"
        ))
        try requireSuccess(response, data: data)
        return (try JSONSerialization.jsonObject(with: data) as? [[String: Any]] ?? []).count
    }

    func thumbnailData(for asset: RemoteAsset) async throws -> Data? {
        guard let thumbnail = asset.thumbnail, thumbnail.storageEncoding == "plain-v1" else { return nil }
        let (data, response) = try await URLSession.shared.data(for: authorized(
            url: serverURL.appending(path: thumbnail.contentPath),
            method: "GET"
        ))
        try requireSuccess(response, data: data)
        return data
    }

    func trash(asset: RemoteAsset) async throws {
        try await send(path: "/v1/assets/\(asset.assetId)/trash", method: "POST")
    }

    func restoreFromTrash(asset: RemoteAsset) async throws {
        try await send(path: "/v1/assets/\(asset.assetId)/restore", method: "POST")
    }

    func restoreToPhotos(asset: RemoteAsset) async throws -> String {
        guard let resource = asset.primary else { throw RemoteLibraryError.noPrimaryResource }
        guard resource.storageEncoding == "plain-v1" else { throw RemoteLibraryError.legacyEncryptedResource }
        let suffix = URL(fileURLWithPath: resource.filename).pathExtension
        let temporary = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension(suffix.isEmpty ? "bin" : suffix)
        defer { try? FileManager.default.removeItem(at: temporary) }

        let (downloaded, response) = try await URLSession.shared.download(for: authorized(
            url: serverURL.appending(path: resource.contentPath),
            method: "GET"
        ))
        try requireSuccess(response, data: Data())
        try FileManager.default.moveItem(at: downloaded, to: temporary)
        try await PHPhotoLibrary.shared().performChanges {
            if resource.mimeType.hasPrefix("video/") {
                PHAssetChangeRequest.creationRequestForAssetFromVideo(atFileURL: temporary)
            } else {
                PHAssetChangeRequest.creationRequestForAssetFromImage(atFileURL: temporary)
            }
        }
        return resource.filename
    }

    private func send(path: String, method: String) async throws {
        let (data, response) = try await URLSession.shared.data(for: authorized(
            url: serverURL.appending(path: path),
            method: method
        ))
        try requireSuccess(response, data: data)
    }

    private func sendJSON(path: String, method: String, body: [String: Any]) async throws {
        var request = authorized(url: serverURL.appending(path: path), method: method)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request)
        try requireSuccess(response, data: data)
    }

    private func authorized(url: URL, method: String) -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    private func requireSuccess(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw RemoteLibraryError.server(String(decoding: data, as: UTF8.self))
        }
    }
}

private struct RemoteTag: Decodable {
    let tagId: UUID
    let name: String
}

enum RemoteLibraryError: LocalizedError {
    case invalidURL
    case noPrimaryResource
    case legacyEncryptedResource
    case invalidCursor
    case server(String)

    var errorDescription: String? {
        switch self {
        case .invalidURL: "服务器地址无效"
        case .noPrimaryResource: "该资产没有可恢复的原始资源"
        case .legacyEncryptedResource: "旧版加密资源需要旧客户端解密后重新上传"
        case .invalidCursor: "服务器返回了无效的分页游标"
        case .server(let message): message.isEmpty ? "服务器请求失败" : message
        }
    }
}
