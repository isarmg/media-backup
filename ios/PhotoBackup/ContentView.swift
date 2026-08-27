import SwiftUI

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

            VStack(alignment: .leading, spacing: 22) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("私有照片仓库")
                        .font(.system(size: 38, weight: .black, design: .rounded))
                    Text("原始画质 · 端到端加密 · 可中断续传")
                        .foregroundStyle(Color(red: 0.18, green: 0.34, blue: 0.28))
                }

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
                .padding(22)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24))
            }
            .padding(24)
        }
    }
}
