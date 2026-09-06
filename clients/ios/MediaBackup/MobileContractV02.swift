import Foundation

enum MobileContractV02 {
    static let product = "media-backup"
    static let applicationVersion = "0.2.1"
    static let revision: UInt32 = 1
    static let stateEpoch = "media-backup-mobile-v0.2-r2"

    static let databaseFilename = "agent-v0.2-r2.sqlite"
    static let stagingDirectory = "backup-staging-v0.2-r2"
    static let keychainService = "org.sarmg.mediabackup.r2.keychain"
    static let preferences = UserDefaults(suiteName: "org.sarmg.mediabackup.r2.preferences")!
    static let tokenKey = "bearer_token_v0_2_r2"
    static let processingTask = "org.sarmg.mediabackup.processing.v0-2-r2"
    static let uploadSession = "org.sarmg.mediabackup.upload.v0-2-r2"

    static func requireIdentity(
        product: String,
        applicationVersion: String,
        revision: UInt32,
        stateEpoch: String
    ) throws {
        guard product == self.product,
              applicationVersion == self.applicationVersion,
              revision == self.revision,
              stateEpoch == self.stateEpoch else {
            throw AgentFailure.message("Rust Agent 返回了非 v0.2 revision 1 合约")
        }
    }
}
