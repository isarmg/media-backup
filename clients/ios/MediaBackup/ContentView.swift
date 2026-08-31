import SwiftUI
import UIKit

struct ContentView: View {
    @EnvironmentObject private var coordinator: BackupCoordinator

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color(red: 0.96, green: 0.89, blue: 0.78), Color(red: 0.85, green: 0.94, blue: 0.89)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("私有照片仓库")
                            .font(.system(size: 38, weight: .black, design: .rounded))
                        Text("HTTPS 加密传输 · 服务端原始文件 · 可中断续传")
                            .foregroundStyle(Color(red: 0.18, green: 0.34, blue: 0.28))
                    }

                    connectionCard
                    albumCard
                    libraryCard
                }
                .padding(24)
            }
        }
        .task { await coordinator.refreshAlbums() }
    }

    private var connectionCard: some View {
        VStack(spacing: 16) {
            TextField("服务端 HTTPS 地址", text: $coordinator.serverURL)
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                .textFieldStyle(.roundedBorder)
            TextField("登录账号", text: $coordinator.username)
                .textInputAutocapitalization(.never)
                .textFieldStyle(.roundedBorder)
            SecureField("登录密码", text: $coordinator.password)
                .textFieldStyle(.roundedBorder)
            Button {
                Task { await coordinator.runBackup() }
            } label: {
                HStack {
                    if coordinator.running { ProgressView().tint(.white) }
                    Text(coordinator.running ? "正在备份" : "立即备份")
                        .fontWeight(.bold)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
            }
            .buttonStyle(.borderedProminent)
            .tint(Color(red: 0.12, green: 0.32, blue: 0.26))
            .disabled(coordinator.running)
            Text(coordinator.status)
                .font(.footnote)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .cardStyle()
    }

    private var albumCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("设备相册").font(.headline)
                    Text("关闭的相册不会备份；选中的相册会单向同步到服务器")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("刷新") { Task { await coordinator.refreshAlbums() } }
            }
            if coordinator.albums.isEmpty {
                Text("授权照片访问后显示可选相册")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            ForEach(coordinator.albums.prefix(24)) { album in
                Toggle(isOn: Binding(
                    get: { coordinator.selectedAlbumIds.contains(album.id) },
                    set: { coordinator.setAlbum(album.id, enabled: $0) }
                )) {
                    VStack(alignment: .leading) {
                        Text(album.name)
                        Text("\(album.count) 项")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .tint(Color(red: 0.12, green: 0.42, blue: 0.34))
            }
        }
        .cardStyle()
    }

    private var libraryCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(coordinator.showingTrash ? "服务器回收站" : "服务器时间线")
                    .font(.headline)
                Text("登录新设备后可直接下载原始文件恢复")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            HStack {
                Button(coordinator.libraryLoading ? "加载中" : "刷新") {
                    Task { await coordinator.refreshLibrary() }
                }
                .disabled(coordinator.libraryLoading)
                Button(coordinator.showingTrash ? "返回时间线" : "查看回收站") {
                    Task { await coordinator.refreshLibrary(trashed: !coordinator.showingTrash) }
                }
                if !coordinator.showingTrash && !coordinator.remoteAssets.isEmpty {
                    Button("全部恢复") { Task { await coordinator.restoreAllToPhone() } }
                }
            }
            .buttonStyle(.bordered)
            Text(coordinator.duplicateGroupCount == 0
                ? "未发现重复组"
                : "发现 \(coordinator.duplicateGroupCount) 组重复内容，可将多余副本移入回收站")
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("要添加的标签", text: $coordinator.newTagName)
                .textFieldStyle(.roundedBorder)

            if coordinator.remoteAssets.isEmpty {
                Text("尚未加载或没有媒体")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            ForEach(coordinator.remoteAssets.prefix(24)) { asset in
                Divider()
                VStack(alignment: .leading, spacing: 7) {
                    if let data = coordinator.remoteThumbnails[asset.id], let image = UIImage(data: data) {
                        Image(uiImage: image)
                            .resizable()
                            .scaledToFill()
                            .frame(width: 88, height: 72)
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                    }
                    Text(asset.primary?.filename ?? "媒体资源")
                        .fontWeight(.semibold)
                        .lineLimit(1)
                    Text(Date(timeIntervalSince1970: Double(asset.sourceCreatedAtMs) / 1000).formatted())
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if !asset.tagNames.isEmpty {
                        Text(asset.tagNames.joined(separator: " · "))
                            .font(.caption)
                            .foregroundStyle(Color(red: 0.12, green: 0.42, blue: 0.34))
                    }
                    HStack {
                        if !asset.isTrashed {
                            Button("恢复到手机") { Task { await coordinator.restoreToPhone(asset) } }
                            Button(asset.favorite ? "取消收藏" : "收藏") {
                                Task { await coordinator.toggleFavorite(asset) }
                            }
                            Button(asset.archived ? "取消归档" : "归档") {
                                Task { await coordinator.toggleArchived(asset) }
                            }
                            if !coordinator.newTagName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                Button("加标签") { Task { await coordinator.addTag(to: asset) } }
                            }
                            Button("回收站", role: .destructive) { Task { await coordinator.toggleTrash(asset) } }
                        } else {
                            Button("撤销删除") { Task { await coordinator.toggleTrash(asset) } }
                        }
                    }
                    .buttonStyle(.borderless)
                    .font(.footnote)
                }
            }
        }
        .cardStyle()
    }
}

private extension View {
    func cardStyle() -> some View {
        padding(20)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 22))
    }
}
