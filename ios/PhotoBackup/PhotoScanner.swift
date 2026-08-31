import Foundation
import Photos
import UIKit
import UniformTypeIdentifiers

struct PhotoAlbum: Identifiable, Hashable {
    let id: String
    let name: String
    let count: Int
}

struct PhotoScanResult {
    let queued: Int
    let albums: [String: (name: String, assetIds: Set<String>)]
}

struct PhotoScanner {
    func requestAccess() async -> PHAuthorizationStatus {
        await PHPhotoLibrary.requestAuthorization(for: .readWrite)
    }

    func albums() -> [PhotoAlbum] {
        var result: [PhotoAlbum] = []
        let collections = PHAssetCollection.fetchAssetCollections(with: .album, subtype: .any, options: nil)
        collections.enumerateObjects { collection, _, _ in
            let count = PHAsset.fetchAssets(in: collection, options: nil).count
            if count > 0 {
                result.append(PhotoAlbum(id: collection.localIdentifier, name: collection.localizedTitle ?? "未命名相册", count: count))
            }
        }
        let smart = PHAssetCollection.fetchAssetCollections(with: .smartAlbum, subtype: .smartAlbumUserLibrary, options: nil)
        smart.enumerateObjects { collection, _, _ in
            result.append(PhotoAlbum(
                id: collection.localIdentifier,
                name: collection.localizedTitle ?? "所有照片",
                count: PHAsset.fetchAssets(in: collection, options: nil).count
            ))
        }
        return result.sorted { $0.count == $1.count ? $0.name < $1.name : $0.count > $1.count }
    }

    func scan(agent: RustAgent, stagingRoot: URL, selectedAlbumIds: Set<String>) async throws -> PhotoScanResult {
        let sourceRoot = stagingRoot.appendingPathComponent("sources", isDirectory: true)
        try FileManager.default.createDirectory(at: sourceRoot, withIntermediateDirectories: true)
        let available = albums()
        let selected = selectedAlbumIds
        var assetsById: [String: PHAsset] = [:]
        var membership: [String: (name: String, assetIds: Set<String>)] = [:]
        for album in available where selected.contains(album.id) {
            let collections = PHAssetCollection.fetchAssetCollections(withLocalIdentifiers: [album.id], options: nil)
            guard let collection = collections.firstObject else { continue }
            let assets = PHAsset.fetchAssets(in: collection, options: nil)
            var ids = Set<String>()
            assets.enumerateObjects { asset, _, _ in
                assetsById[asset.localIdentifier] = asset
                ids.insert(asset.localIdentifier)
            }
            membership[album.id] = (album.name, ids)
        }
        let ordered = assetsById.values.sorted {
            ($0.creationDate ?? .distantPast) < ($1.creationDate ?? .distantPast)
        }
        var queued = 0
        for asset in ordered {
            try Task.checkCancellation()
            let modifiedMs = Int64((asset.modificationDate ?? asset.creationDate ?? .distantPast).timeIntervalSince1970 * 1000)
            let createdMs = Int64((asset.creationDate ?? .distantPast).timeIntervalSince1970 * 1000)
            let metadata = try JSONSerialization.data(withJSONObject: [
                "pixel_width": asset.pixelWidth,
                "pixel_height": asset.pixelHeight,
                "duration": asset.duration,
                "favorite": asset.isFavorite,
                "hidden": asset.isHidden,
            ])
            for resource in PHAssetResource.assetResources(for: asset) {
                let resourceId = "\(resource.type.rawValue):\(resource.originalFilename)"
                guard agent.needs(asset: asset.localIdentifier, resource: resourceId, modifiedMs: modifiedMs) else { continue }
                let output = sourceRoot.appendingPathComponent(UUID().uuidString)
                try await export(resource, to: output)
                let size = ((try FileManager.default.attributesOfItem(atPath: output.path)[.size]) as? NSNumber)?.uint64Value ?? 0
                try agent.enqueue(EnqueueInput(
                    product: MobileContractV02.product,
                    applicationVersion: MobileContractV02.applicationVersion,
                    revision: MobileContractV02.revision,
                    stateEpoch: MobileContractV02.stateEpoch,
                    sourceAssetId: asset.localIdentifier,
                    sourceResourceId: resourceId,
                    mediaKind: asset.mediaType == .video ? "video" : "photo",
                    role: resource.type == .photo || resource.type == .video ? "primary" : "resource-\(resource.type.rawValue)",
                    filePath: output.path,
                    filename: resource.originalFilename,
                    mimeType: UTType(resource.uniformTypeIdentifier)?.preferredMIMEType ?? "application/octet-stream",
                    sourceCreatedAtMs: createdMs,
                    modifiedMs: modifiedMs,
                    sourceSize: size,
                    metadataJson: String(decoding: metadata, as: UTF8.self),
                    removeSourceAfterPrepare: true
                ))
                queued += 1
            }
            let thumbnailId = "\(asset.localIdentifier)#thumbnail-v1"
            if agent.needs(asset: asset.localIdentifier, resource: thumbnailId, modifiedMs: modifiedMs),
               let thumbnail = try await thumbnail(for: asset) {
                let output = sourceRoot.appendingPathComponent("\(UUID().uuidString).thumbnail.jpg")
                try thumbnail.write(to: output, options: .atomic)
                try agent.enqueue(EnqueueInput(
                    product: MobileContractV02.product,
                    applicationVersion: MobileContractV02.applicationVersion,
                    revision: MobileContractV02.revision,
                    stateEpoch: MobileContractV02.stateEpoch,
                    sourceAssetId: asset.localIdentifier,
                    sourceResourceId: thumbnailId,
                    mediaKind: asset.mediaType == .video ? "video" : "photo",
                    role: "thumbnail",
                    filePath: output.path,
                    filename: "thumbnail.jpg",
                    mimeType: "image/jpeg",
                    sourceCreatedAtMs: createdMs,
                    modifiedMs: modifiedMs,
                    sourceSize: UInt64(thumbnail.count),
                    metadataJson: "{\"thumbnail_of\":\"\(asset.localIdentifier)\"}",
                    removeSourceAfterPrepare: true
                ))
                queued += 1
            }
        }
        return PhotoScanResult(queued: queued, albums: membership)
    }

    private func export(_ resource: PHAssetResource, to url: URL) async throws {
        let options = PHAssetResourceRequestOptions()
        options.isNetworkAccessAllowed = true
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            PHAssetResourceManager.default().writeData(for: resource, toFile: url, options: options) { error in
                if let error { continuation.resume(throwing: error) }
                else { continuation.resume() }
            }
        }
    }

    private func thumbnail(for asset: PHAsset) async throws -> Data? {
        let options = PHImageRequestOptions()
        options.isNetworkAccessAllowed = true
        options.deliveryMode = .highQualityFormat
        return try await withCheckedThrowingContinuation { continuation in
            PHImageManager.default().requestImage(
                for: asset,
                targetSize: CGSize(width: 512, height: 512),
                contentMode: .aspectFit,
                options: options
            ) { image, info in
                if (info?[PHImageResultIsDegradedKey] as? Bool) == true { return }
                if let error = info?[PHImageErrorKey] as? Error { continuation.resume(throwing: error) }
                else { continuation.resume(returning: image?.jpegData(compressionQuality: 0.82)) }
            }
        }
    }
}
