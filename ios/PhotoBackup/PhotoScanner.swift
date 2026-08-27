import Foundation
import Photos
import UniformTypeIdentifiers

struct PhotoScanner {
    func requestAccess() async -> PHAuthorizationStatus {
        await PHPhotoLibrary.requestAuthorization(for: .readWrite)
    }

    func scan(agent: RustAgent, stagingRoot: URL) async throws -> Int {
        let sourceRoot = stagingRoot.appendingPathComponent("sources", isDirectory: true)
        try FileManager.default.createDirectory(at: sourceRoot, withIntermediateDirectories: true)
        let options = PHFetchOptions()
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: true)]
        let assets = PHAsset.fetchAssets(with: options)
        var queued = 0
        for position in 0..<assets.count {
            try Task.checkCancellation()
            let asset = assets.object(at: position)
            let modifiedMs = Int64((asset.modificationDate ?? asset.creationDate ?? .distantPast).timeIntervalSince1970 * 1000)
            let createdMs = Int64((asset.creationDate ?? .distantPast).timeIntervalSince1970 * 1000)
            for resource in PHAssetResource.assetResources(for: asset) {
                let resourceId = "\(resource.type.rawValue):\(resource.originalFilename)"
                guard agent.needs(asset: asset.localIdentifier, resource: resourceId, modifiedMs: modifiedMs) else { continue }
                let output = sourceRoot.appendingPathComponent(UUID().uuidString)
                try await export(resource, to: output)
                let attributes = try FileManager.default.attributesOfItem(atPath: output.path)
                let size = (attributes[.size] as? NSNumber)?.uint64Value ?? 0
                let metadata = try JSONSerialization.data(withJSONObject: [
                    "pixel_width": asset.pixelWidth,
                    "pixel_height": asset.pixelHeight,
                    "duration": asset.duration,
                    "favorite": asset.isFavorite,
                    "hidden": asset.isHidden,
                    "resource_type": resource.type.rawValue,
                ])
                let input = EnqueueInput(
                    sourceAssetId: asset.localIdentifier,
                    sourceResourceId: resourceId,
                    mediaKind: asset.mediaType == .video ? "video" : "photo",
                    role: "resource-\(resource.type.rawValue)",
                    filePath: output.path,
                    filename: resource.originalFilename,
                    mimeType: UTType(resource.uniformTypeIdentifier)?.preferredMIMEType ?? "application/octet-stream",
                    sourceCreatedAtMs: createdMs,
                    modifiedMs: modifiedMs,
                    sourceSize: size,
                    metadataJson: String(decoding: metadata, as: UTF8.self),
                    removeSourceAfterPrepare: true
                )
                try agent.enqueue(input)
                queued += 1
            }
        }
        return queued
    }

    private func export(_ resource: PHAssetResource, to url: URL) async throws {
        let options = PHAssetResourceRequestOptions()
        options.isNetworkAccessAllowed = true
        try await withCheckedThrowingContinuation { continuation in
            PHAssetResourceManager.default().writeData(for: resource, toFile: url, options: options) { error in
                if let error { continuation.resume(throwing: error) }
                else { continuation.resume() }
            }
        }
    }
}
