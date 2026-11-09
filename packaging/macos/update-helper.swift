import Darwin
import Foundation
import CryptoKit
import AppKit

func processStart(_ pid: pid_t) -> UInt64? {
    var info = proc_bsdinfo()
    let size = Int32(MemoryLayout<proc_bsdinfo>.size)
    let count = withUnsafeMutablePointer(to: &info) { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, $0, size) }
    return count == size ? UInt64(info.pbi_start_tvsec) : nil
}

final class UpdateSocket {
    let fd: Int32
    init(_ fd: Int32) throws {
        guard fd >= 0 else { throw fail("Could not create the local update channel.") }
        self.fd = fd
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        _ = setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var yes: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, socklen_t(MemoryLayout<Int32>.size))
    }
    deinit { close(fd) }
    func write(_ value: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: value)
        guard data.count <= 65536 else { throw fail("Update message too large.") }
        var length = UInt32(data.count).bigEndian
        let header = withUnsafeBytes(of: &length) { Data($0) }
        for part in [header, data] {
            try part.withUnsafeBytes { bytes in
                var offset = 0
                while offset < bytes.count {
                    guard let base = bytes.baseAddress else { throw fail("Invalid update message.") }
                    let count = Darwin.send(fd, base.advanced(by: offset), bytes.count - offset, 0)
                    guard count > 0 else { throw fail("The update channel was closed.") }
                    offset += count
                }
            }
        }
    }
    func readBytes(_ count: Int) throws -> Data {
        var bytes = [UInt8](repeating: 0, count: count)
        try bytes.withUnsafeMutableBytes { buffer in
            var offset = 0
            while offset < count {
                guard let base = buffer.baseAddress else { throw fail("Invalid update message.") }
                let received = Darwin.recv(fd, base.advanced(by: offset), count - offset, 0)
                guard received > 0 else { throw fail("The update channel was closed.") }
                offset += received
            }
        }
        return Data(bytes)
    }
    func read() throws -> [String: Any] {
        let header = try readBytes(4)
        let count = header.reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        guard count <= 65536 else { throw fail("Update message too large.") }
        guard let object = try JSONSerialization.jsonObject(with: readBytes(Int(count))) as? [String: Any] else { throw fail("Invalid update message.") }
        return object
    }
}

func localAddress(_ port: UInt16) -> sockaddr_in {
    var address = sockaddr_in()
    address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
    return address
}

func resolveCapability(_ reference: String) throws -> String {
    let token: String
    if reference.hasPrefix("@") {
        let file = open(String(reference.dropFirst()), O_RDONLY | O_NOFOLLOW | O_NONBLOCK)
        guard file >= 0 else { throw fail("Could not read the private update capability.") }
        let handle = FileHandle(fileDescriptor: file, closeOnDealloc: true)
        var metadata = stat()
        guard fstat(file, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG, metadata.st_size <= 64,
              let bytes = try handle.read(upToCount: 65), let value = String(data: bytes, encoding: .utf8) else { throw fail("Invalid private update capability.") }
        token = value
    } else { token = reference }
    guard token.utf8.count == 64, token.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { throw fail("Invalid private update capability.") }
    return token
}

func controllerSocket(_ endpoint: String, token reference: String) throws -> UpdateSocket {
    let token = try resolveCapability(reference)
    let pieces = endpoint.split(separator: ":")
    guard pieces.count == 2, pieces[0] == "127.0.0.1", let port = UInt16(pieces[1]), token.count == 64 else { throw fail("Invalid local update endpoint.") }
    let channel = try UpdateSocket(socket(AF_INET, SOCK_STREAM, 0))
    var address = localAddress(port)
    let result = withUnsafePointer(to: &address) { pointer in pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(channel.fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
    guard result == 0 else { throw fail("Could not connect to RustZIP.") }
    try channel.write(["type": "Hello", "token": token, "pid": getpid(), "started": processStart(getpid()) ?? 0])
    return channel
}

func receiptListener() throws -> (UpdateSocket, String) {
    let server = try UpdateSocket(socket(AF_INET, SOCK_STREAM, 0))
    var address = localAddress(0)
    let result = withUnsafePointer(to: &address) { pointer in pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(server.fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
    guard result == 0, listen(server.fd, 8) == 0 else { throw fail("Could not prepare the update startup channel.") }
    var length = socklen_t(MemoryLayout<sockaddr_in>.size)
    let status = withUnsafeMutablePointer(to: &address) { pointer in pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(server.fd, $0, &length) } }
    guard status == 0 else { throw fail("Could not read the update startup endpoint.") }
    return (server, "127.0.0.1:\(UInt16(bigEndian: address.sin_port))")
}

struct UpdateJournal: Codable {
    var id: String
    var oldVersion: String
    var newVersion: String
    var phase: String
    var updaterPID: Int32
    var updaterStart: UInt64
}

func journalURL(_ app: URL) -> URL { app.deletingLastPathComponent().appendingPathComponent(".\(app.lastPathComponent).rustzip-update.json") }
func journalPaths(_ journal: UpdateJournal, app: URL) -> (URL, URL, URL) {
    let parent = app.deletingLastPathComponent()
    let directory = parent.appendingPathComponent(".RustZIP-update-\(journal.id)")
    return (directory, directory.appendingPathComponent("RustZIP.app"), parent.appendingPathComponent(".RustZIP-backup-\(journal.id).app"))
}

func persistJournal(_ journal: UpdateJournal, app: URL) throws {
    let destination = journalURL(app)
    try JSONEncoder().encode(journal).write(to: destination, options: .atomic)
    let file = open(destination.path, O_RDONLY | O_NOFOLLOW)
    guard file >= 0 else { throw fail("Could not persist the update transaction.") }
    defer { close(file) }
    guard fsync(file) == 0 else { throw fail("Could not persist the update transaction.") }
    let directory = open(app.deletingLastPathComponent().path, O_RDONLY)
    guard directory >= 0 else { throw fail("Could not persist the update transaction directory.") }
    defer { close(directory) }
    guard fsync(directory) == 0 else { throw fail("Could not persist the update transaction directory.") }
}

func readJournal(_ app: URL) throws -> UpdateJournal {
    let file = journalURL(app)
    let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
    guard values.isRegularFile == true, values.isSymbolicLink != true, (values.fileSize ?? 65537) <= 65536 else { throw fail("Invalid update transaction.") }
    let journal = try JSONDecoder().decode(UpdateJournal.self, from: Data(contentsOf: file))
    guard UUID(uuidString: journal.id)?.uuidString == journal.id,
          ["prepared", "swapped", "confirmed"].contains(journal.phase),
          !journal.oldVersion.isEmpty, !journal.newVersion.isEmpty else { throw fail("Invalid update transaction.") }
    return journal
}

func swapApps(_ first: URL, _ second: URL) throws {
    guard renamex_np(first.path, second.path, UInt32(RENAME_SWAP)) == 0 else { throw fail("Could not atomically replace RustZIP. The original app was retained.") }
}

func cleanupJournal(_ journal: UpdateJournal, app: URL) throws {
    let (directory, _, backup) = journalPaths(journal, app: app)
    if FileManager.default.fileExists(atPath: backup.path) { try FileManager.default.removeItem(at: backup) }
    if FileManager.default.fileExists(atPath: directory.path) { try FileManager.default.removeItem(at: directory) }
    try FileManager.default.removeItem(at: journalURL(app))
}

func recoverTransaction(_ app: URL, expectedTeam: String?) throws {
    guard FileManager.default.fileExists(atPath: journalURL(app).path) else { return }
    let journal = try readJournal(app)
    guard processStart(journal.updaterPID) != journal.updaterStart else { throw fail("Another RustZIP update is still running.") }
    if journal.phase == "confirmed" { try cleanupJournal(journal, app: app); return }
    try rollbackTransaction(journal, app: app, expectedTeam: expectedTeam)
}

func rollbackTransaction(_ journal: UpdateJournal, app: URL, expectedTeam: String?) throws {
    let (_, staged, backup) = journalPaths(journal, app: app)
    if try bundleValue(app, "CFBundleShortVersionString") == journal.newVersion {
        let previous = FileManager.default.fileExists(atPath: backup.path) ? backup : staged
        guard try bundleValue(previous, "CFBundleIdentifier") == bundleValue(app, "CFBundleIdentifier"),
              try bundleValue(previous, "CFBundleShortVersionString") == journal.oldVersion else { throw fail("The previous app could not be verified. The backup was retained.") }
        try validateSignature(previous, expectedTeam: expectedTeam)
        try swapApps(app, previous)
    }
    guard try bundleValue(app, "CFBundleShortVersionString") == journal.oldVersion else { throw fail("The previous app could not be restored.") }
    try cleanupJournal(journal, app: app)
}

final class UpdateLock {
    let fd: Int32
    init(app: URL) throws {
        fd = open(journalURL(app).appendingPathExtension("lock").path, O_RDWR | O_CREAT | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { throw fail("Could not open the update lock.") }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { close(fd); throw fail("Another RustZIP update is already in progress.") }
    }
    deinit { close(fd) }
}

struct UpdateFailure: Error, LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

struct ReleaseVersion: Comparable {
    let core: [UInt64]
    let prerelease: [String]
    init(_ value: String) throws {
        guard value.utf8.count <= 128 else { throw fail("Invalid update version.") }
        let build = value.split(separator: "+", maxSplits: 1, omittingEmptySubsequences: false)
        let parts = build[0].split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        let numbers = parts[0].split(separator: ".", omittingEmptySubsequences: false)
        guard numbers.count == 3 else { throw fail("Invalid update version.") }
        var parsed: [UInt64] = []
        for number in numbers {
            guard !number.isEmpty, number.utf8.allSatisfy({ (48...57).contains($0) }),
                  number.count == 1 || number.first != "0", let integer = UInt64(number) else { throw fail("Invalid update version.") }
            parsed.append(integer)
        }
        core = parsed
        prerelease = parts.count == 2 ? parts[1].split(separator: ".", omittingEmptySubsequences: false).map(String.init) : []
        let metadata = build.count == 2 ? build[1].split(separator: ".", omittingEmptySubsequences: false).map(String.init) : []
        for identifier in prerelease + metadata {
            guard !identifier.isEmpty, identifier.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 }) else { throw fail("Invalid update version.") }
        }
        for identifier in prerelease where identifier.utf8.allSatisfy({ (48...57).contains($0) }) {
            guard identifier.count == 1 || identifier.first != "0" else { throw fail("Invalid update version.") }
        }
    }
    static func < (a: ReleaseVersion, b: ReleaseVersion) -> Bool {
        for (left, right) in zip(a.core, b.core) where left != right { return left < right }
        if a.prerelease.isEmpty || b.prerelease.isEmpty { return !a.prerelease.isEmpty && b.prerelease.isEmpty }
        for (left, right) in zip(a.prerelease, b.prerelease) where left != right {
            let leftNumeric = left.utf8.allSatisfy { (48...57).contains($0) }
            let rightNumeric = right.utf8.allSatisfy { (48...57).contains($0) }
            if leftNumeric != rightNumeric { return leftNumeric }
            if leftNumeric && left.count != right.count { return left.count < right.count }
            return left < right
        }
        return a.prerelease.count < b.prerelease.count
    }
}

func fail(_ message: String) -> UpdateFailure {
    UpdateFailure(message: message)
}

@discardableResult
func run(_ executable: String, _ arguments: [String], captureOutput: Bool = false) throws -> String {
    let process = Process()
    let output: Pipe? = captureOutput ? Pipe() : nil
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = output ?? FileHandle.nullDevice
    process.standardError = output ?? FileHandle.nullDevice
    try process.run()
    var data = Data()
    if let handle = output?.fileHandleForReading {
        while let chunk = try handle.read(upToCount: 65536), !chunk.isEmpty {
            guard data.count + chunk.count <= 8 * 1024 * 1024 else {
                process.terminate()
                throw fail("Update metadata exceeds its size limit.")
            }
            data.append(chunk)
        }
    }
    process.waitUntilExit()
    let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    guard process.terminationStatus == 0 else {
        throw fail(text.isEmpty ? "A required macOS update operation failed." : text)
    }
    return text
}

func bundleValue(_ app: URL, _ key: String) throws -> String {
    let info = app.appendingPathComponent("Contents/Info.plist")
    let data = try Data(contentsOf: info)
    guard
        let dictionary = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
        let value = dictionary[key] as? String
    else {
        throw fail("The downloaded app is missing required bundle metadata.")
    }
    return value
}

func signingTeam(_ app: URL) throws -> String? {
    let details = try run("/usr/bin/codesign", ["-dv", "--verbose=4", app.path], captureOutput: true)
    guard let line = details.split(separator: "\n").first(where: { $0.hasPrefix("TeamIdentifier=") }) else {
        return nil
    }
    let value = String(line.dropFirst("TeamIdentifier=".count))
    return value == "not set" || value.isEmpty ? nil : value
}

func validateSignature(_ app: URL, expectedTeam: String?) throws {
    try run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app.path], captureOutput: true)
    let actualTeam = try signingTeam(app)
    guard actualTeam == expectedTeam else {
        throw fail("The downloaded app was signed by a different Apple Developer account.")
    }
}

func shellQuote(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

func appleScriptQuote(_ value: String) -> String {
    value.replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"")
}

func helperText(_ key: Int, app: URL, language: String) -> String {
    let strings = app.appendingPathComponent("Contents/Resources/update-helper-strings.tsv")
    guard
        let contents = try? String(contentsOf: strings, encoding: .utf8),
        let row = contents.split(whereSeparator: \.isNewline).first(where: {
            let columns = $0.split(separator: "\t", omittingEmptySubsequences: false)
            return columns.count == 3 && columns[0] == Substring(language)
        })
    else {
        return key == 1 ? "RustZIP needs permission to update its copy in Applications." : "RustZIP could not complete the update: {detail}"
    }
    let columns = row.split(separator: "\t", omittingEmptySubsequences: false)
    return String(columns[key])
}

func isWritable(_ directory: URL) -> Bool {
    access(directory.path, W_OK) == 0
}

func elevate(helper: URL, app: URL, archive: URL, format: String, sha256: String,
             source: URL, version: String, platform: String, signature: String,
             language: String, uid: uid_t) throws {
    let command = [
        helper.path, "--elevated", "--app", app.path, "--archive", archive.path,
        "--format", format, "--sha256", sha256, "--prepared-app", source.path,
        "--version", version, "--platform", platform, "--signature", signature,
        "--language", language, "--pid", "0", "--uid", String(uid)
    ].map(shellQuote).joined(separator: " ")
    let prompt = appleScriptQuote(helperText(1, app: app, language: language))
    let script = "do shell script \"\(appleScriptQuote(command))\" with administrator privileges with prompt \"\(prompt)\""
    try run("/usr/bin/osascript", ["-e", script])
}

func mountImage(_ dmg: URL) throws -> URL {
    let mount = URL(fileURLWithPath: "/private/tmp/RustZIPUpdate-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: mount, withIntermediateDirectories: false)
    do {
        try run("/usr/bin/hdiutil", ["attach", "-readonly", "-nobrowse", "-quiet", "-mountpoint", mount.path, dmg.path])
        return mount
    } catch {
        try? FileManager.default.removeItem(at: mount)
        throw error
    }
}

func updatedApp(in directory: URL, bundleID: String) throws -> URL {
    let children = try FileManager.default.contentsOfDirectory(
        at: directory, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    var match: URL?
    for child in children where child.pathExtension == "app" {
        let values = try child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        if values.isDirectory == true && values.isSymbolicLink != true
            && (try? bundleValue(child, "CFBundleIdentifier")) == bundleID {
            guard match == nil else {
                throw fail("The downloaded update contains multiple RustZIP apps.")
            }
            match = child
        }
    }
    guard let app = match else {
        throw fail("The downloaded update does not contain RustZIP.")
    }
    return app
}

func verifyArchive(_ archive: URL, expectedSHA256: String) throws {
    let metadata = try archive.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey, .isSymbolicLinkKey])
    guard metadata.isRegularFile == true, metadata.isSymbolicLink != true,
          let size = metadata.fileSize, size > 0, size <= 512 * 1024 * 1024 else { throw fail("The update archive exceeds its size limit.") }
    let expected = expectedSHA256.lowercased()
    guard expected.count == 64,
          expected.unicodeScalars.allSatisfy({ CharacterSet(charactersIn: "0123456789abcdef").contains($0) })
    else {
        throw fail("The update checksum is invalid.")
    }
    let output = try run("/usr/bin/shasum", ["-a", "256", archive.path], captureOutput: true)
    guard output.split(separator: " ").first.map(String.init)?.lowercased() == expected else {
        throw fail("The downloaded update changed after signature verification.")
    }
}

func decodeHex(_ value: String, length: Int) throws -> Data {
    guard value.utf8.count == length * 2,
          value.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else {
        throw fail("Invalid update signature metadata.")
    }
    var result = Data()
    var index = value.startIndex
    for _ in 0..<length {
        let end = value.index(index, offsetBy: 2)
        guard let byte = UInt8(value[index..<end], radix: 16) else {
            throw fail("Invalid update signature metadata.")
        }
        result.append(byte)
        index = end
    }
    return result
}

func verifyReleaseSignature(version: String, platform: String, format: String,
                            sha256: String, signature: String) throws {
    #if arch(arm64)
    let basePlatform = "macos-aarch64"
    #elseif arch(x86_64)
    let basePlatform = "macos-x86_64"
    #else
    throw fail("Unsupported update architecture.")
    #endif
    let expectedPlatform = format == "zip" ? basePlatform + "-zip" : basePlatform
    guard (format == "zip" || format == "dmg") && platform == expectedPlatform else {
        throw fail("The update targets a different platform.")
    }
    #if RUSTZIP_UPDATER_TESTING
    let keyHex = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
    #else
    let keyHex = "55df9ad8f16242712a84fb7ffed722ed5c99ac90c5007511c780d6bec5bea894"
    #endif
    _ = try decodeHex(sha256, length: 32)
    let key = try Curve25519.Signing.PublicKey(rawRepresentation: decodeHex(keyHex, length: 32))
    let message = Data("rustzip-update-v1\n\(version)\n\(platform)\n\(sha256)".utf8)
    guard key.isValidSignature(try decodeHex(signature, length: 64), for: message) else {
        throw fail("The update release signature is invalid.")
    }
}

func withUpdateDirectory(_ archive: URL, format: String, body: (URL) throws -> Void) throws {
    switch format {
    case "dmg":
        let mount = try mountImage(archive)
        defer {
            _ = try? run("/usr/bin/hdiutil", ["detach", "-quiet", mount.path])
            try? FileManager.default.removeItem(at: mount)
        }
        try body(mount)
    case "zip":
        let summary = try run("/usr/bin/zipinfo", ["-t", archive.path], captureOutput: true)
        let expression = try NSRegularExpression(pattern: "([0-9]+) files?,[ \\t]*([0-9]+) bytes uncompressed")
        guard let match = expression.firstMatch(in: summary, range: NSRange(summary.startIndex..., in: summary)),
              let fileRange = Range(match.range(at: 1), in: summary), let sizeRange = Range(match.range(at: 2), in: summary),
              let files = UInt64(summary[fileRange]), let size = UInt64(summary[sizeRange]),
              files <= 100000, size <= 2 * 1024 * 1024 * 1024 else { throw fail("The update ZIP exceeds its expansion limit.") }
        let names = try run("/usr/bin/zipinfo", ["-1", archive.path], captureOutput: true)
        for name in names.split(separator: "\n") {
            let parts = name.split(separator: "/")
            guard !name.hasPrefix("/"), !name.contains("\\"), !parts.contains(".."),
                  parts.first == "RustZIP.app" || parts.first == "__MACOSX" else { throw fail("The update ZIP contains an unsafe path.") }
        }
        let directory = archive.deletingLastPathComponent()
            .appendingPathComponent("extracted-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: directory) }
        try run("/usr/bin/ditto", ["-x", "-k", archive.path, directory.path])
        try body(directory)
    default:
        throw fail("The update archive format is unsupported.")
    }
}

func validateAppTree(_ app: URL) throws {
    let root = app.resolvingSymlinksInPath().path
    let keys: Set<URLResourceKey> = [.fileResourceTypeKey, .fileSizeKey]
    guard let enumerator = FileManager.default.enumerator(at: app, includingPropertiesForKeys: Array(keys)) else { throw fail("Could not inspect the updated app.") }
    var count = 0
    var size: UInt64 = 0
    let deadline = Date().addingTimeInterval(120)
    for case let file as URL in enumerator {
        count += 1
        guard count <= 100000, file.pathComponents.count - app.pathComponents.count <= 64, Date() < deadline else { throw fail("The updated app exceeds its inspection limit.") }
        let values = try file.resourceValues(forKeys: keys)
        switch values.fileResourceType {
        case .regular:
            size += UInt64(max(0, values.fileSize ?? 0))
            guard size <= 2 * 1024 * 1024 * 1024 else { throw fail("The updated app exceeds its size limit.") }
        case .directory: break
        case .symbolicLink:
            enumerator.skipDescendants()
            let target = file.resolvingSymlinksInPath().path
            guard (target == root || target.hasPrefix(root + "/")), FileManager.default.fileExists(atPath: target) else { throw fail("The updated app contains an escaping or broken link.") }
        default: throw fail("The updated app contains a special file.")
        }
    }
}

func waitForExit(_ pid: pid_t, started: UInt64? = nil) throws {
    guard pid > 0 else { return }
    while true {
        if let started, processStart(pid) != started { return }
        if kill(pid, 0) != 0 && errno == ESRCH { return }
        usleep(100_000)
    }
}

func openInstalledApp(_ app: URL, uid: uid_t, elevated: Bool, arguments: [String] = []) throws {
    var openArguments = ["-n", "-a", app.path]
    if !arguments.isEmpty { openArguments += ["--args"] + arguments }
    if elevated {
        try run("/bin/launchctl", ["asuser", String(uid), "/usr/bin/sudo", "-u", "#\(uid)", "/usr/bin/open"] + openArguments)
    } else {
        try run("/usr/bin/open", openArguments)
    }
}

struct StartupUnconfirmed: Error, LocalizedError {
    var errorDescription: String? { "RustZIP did not confirm startup. The previous app was retained; close RustZIP and launch it again to recover." }
}

func runningApps(_ app: URL) -> [NSRunningApplication] {
    NSWorkspace.shared.runningApplications.filter { !$0.isTerminated && $0.bundleURL?.standardizedFileURL.resolvingSymlinksInPath() == app.standardizedFileURL.resolvingSymlinksInPath() }
}

func awaitStartup(_ app: URL, journal: UpdateJournal, uid: uid_t, elevated: Bool) throws -> UpdateSocket {
    let (server, endpoint) = try receiptListener()
    let token = (UUID().uuidString + UUID().uuidString).replacingOccurrences(of: "-", with: "").lowercased()
    let privateDirectory = URL(fileURLWithPath: "/private/tmp/.RustZIP-startup-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: privateDirectory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    defer { try? FileManager.default.removeItem(at: privateDirectory) }
    let capability = privateDirectory.appendingPathComponent("startup.token")
    try Data(token.utf8).write(to: capability, options: .withoutOverwriting)
    guard chmod(capability.path, 0o600) == 0 else { throw fail("Could not protect the update capability.") }
    if uid != getuid() {
        guard chown(capability.path, uid, gid_t.max) == 0, chown(privateDirectory.path, uid, gid_t.max) == 0 else { throw fail("Could not hand startup confirmation to the current user.") }
    }
    let executable = app.appendingPathComponent("Contents/MacOS").appendingPathComponent(try bundleValue(app, "CFBundleExecutable")).resolvingSymlinksInPath()
    try openInstalledApp(app, uid: uid, elevated: elevated, arguments: ["--rustzip-update-start", endpoint, "@" + capability.path, journal.id, journal.newVersion])
    #if RUSTZIP_UPDATER_TESTING
    let deadline = Date().addingTimeInterval(8)
    #else
    let deadline = Date().addingTimeInterval(45)
    #endif
    while Date() < deadline {
        var descriptor = pollfd(fd: server.fd, events: Int16(POLLIN), revents: 0)
        if poll(&descriptor, 1, 100) <= 0 { continue }
        let accepted = accept(server.fd, nil, nil)
        guard accepted >= 0 else { continue }
        do {
            let channel = try UpdateSocket(accepted)
            let hello = try channel.read()
            guard hello["type"] as? String == "Hello", hello["token"] as? String == token,
                  let pid = (hello["pid"] as? NSNumber)?.int32Value,
                  let started = (hello["started"] as? NSNumber)?.uint64Value,
                  processStart(pid) == started else { continue }
            var path = [UInt8](repeating: 0, count: 4 * Int(MAXPATHLEN))
            let count = path.withUnsafeMutableBytes { proc_pidpath(pid, $0.baseAddress, UInt32($0.count)) }
            guard count > 0 else { continue }
            let actual = URL(fileURLWithPath: String(decoding: path.prefix(while: { $0 != 0 }), as: UTF8.self)).resolvingSymlinksInPath()
            guard actual == executable, try channel.read()["type"] as? String == "Ready" else { continue }
            return channel
        } catch { continue }
    }
    for application in runningApps(app) { _ = application.terminate() }
    let exitDeadline = Date().addingTimeInterval(10)
    while Date() < exitDeadline && !runningApps(app).isEmpty { usleep(100_000) }
    if !runningApps(app).isEmpty { throw StartupUnconfirmed() }
    throw fail("The updated app did not confirm startup. The previous version will be restored.")
}

func installPreparedApp(_ source: URL, app: URL, bundleID: String, expectedTeam: String?,
                        version: String, uid: uid_t, elevated: Bool) throws {
    let parent = app.deletingLastPathComponent()
    let lock = try UpdateLock(app: app)
    defer { withExtendedLifetime(lock) {} }
    try recoverTransaction(app, expectedTeam: expectedTeam)
    guard runningApps(app).isEmpty else { throw fail("Close other RustZIP instances before updating.") }
    guard try bundleValue(source, "CFBundleIdentifier") == bundleID,
          try bundleValue(source, "CFBundleShortVersionString") == version else {
        throw fail("The downloaded app version does not match the signed update manifest.")
    }
    try validateSignature(source, expectedTeam: expectedTeam)
    try validateAppTree(source)

    let id = UUID().uuidString
    let stagingDirectory = parent.appendingPathComponent(".RustZIP-update-\(id)")
    try FileManager.default.createDirectory(
        at: stagingDirectory, withIntermediateDirectories: false,
        attributes: [.posixPermissions: 0o700])
    let staged = stagingDirectory.appendingPathComponent("RustZIP.app")
    let backup = parent.appendingPathComponent(".RustZIP-backup-\(id).app")
    var journal = UpdateJournal(id: id, oldVersion: try bundleValue(app, "CFBundleShortVersionString"), newVersion: version, phase: "prepared", updaterPID: getpid(), updaterStart: processStart(getpid()) ?? 0)
    defer { if !FileManager.default.fileExists(atPath: journalURL(app).path) { try? FileManager.default.removeItem(at: stagingDirectory) } }

    try run("/usr/bin/ditto", ["--rsrc", "--extattr", "--acl", source.path, staged.path])
    try validateSignature(staged, expectedTeam: expectedTeam)
    try validateAppTree(staged)
    guard try bundleValue(staged, "CFBundleIdentifier") == bundleID,
          try bundleValue(staged, "CFBundleShortVersionString") == version else {
        throw fail("The staged app version changed during installation.")
    }

    do {
        try persistJournal(journal, app: app)
        try swapApps(app, staged)
        journal.phase = "swapped"
        try persistJournal(journal, app: app)
        try FileManager.default.moveItem(at: staged, to: backup)
        #if RUSTZIP_UPDATER_TESTING
        if CommandLine.arguments.contains("--fault-after-swap") { _exit(86) }
        #endif
        let receipt = try awaitStartup(app, journal: journal, uid: uid, elevated: elevated)
        journal.phase = "confirmed"
        try persistJournal(journal, app: app)
        try receipt.write(["type": "Committed"])
        try? cleanupJournal(journal, app: app)
    } catch {
        if !(error is StartupUnconfirmed) && journal.phase != "confirmed" {
            if FileManager.default.fileExists(atPath: journalURL(app).path) { try rollbackTransaction(journal, app: app, expectedTeam: expectedTeam) }
            else { try? FileManager.default.removeItem(at: stagingDirectory) }
        }
        throw error
    }
}

func install(app originalApp: URL, archive originalArchive: URL, format: String, sha256: String,
             preparedApp: URL?, version: String, platform: String, signature: String,
             language: String, uid: uid_t, elevated: Bool) throws {
    let app = originalApp.standardizedFileURL.resolvingSymlinksInPath()
    let archive = originalArchive.standardizedFileURL.resolvingSymlinksInPath()
    let bundleID = try bundleValue(app, "CFBundleIdentifier")
    #if RUSTZIP_UPDATER_TESTING
    let expectedBundleID = "com.yuhuotech.rustzip.updater-smoke"
    #else
    let expectedBundleID = "com.yuhuotech.rustzip"
    #endif
    guard bundleID == expectedBundleID else {
        throw fail("The current app is not RustZIP.")
    }
    let expectedTeam = try signingTeam(app)
    try validateSignature(app, expectedTeam: expectedTeam)
    try verifyReleaseSignature(version: version, platform: platform, format: format,
                               sha256: sha256, signature: signature)
    let currentVersion = try ReleaseVersion(bundleValue(app, "CFBundleShortVersionString"))
    let offeredVersion = try ReleaseVersion(version)
    guard offeredVersion > currentVersion, !currentVersion.prerelease.isEmpty || offeredVersion.prerelease.isEmpty else { throw fail("The update is not newer than the installed version.") }

    if let source = preparedApp {
        guard elevated && geteuid() == 0 else {
            throw fail("Only the authorized installer can use a prepared app.")
        }
        try installPreparedApp(source, app: app, bundleID: bundleID, expectedTeam: expectedTeam,
                               version: version, uid: uid, elevated: true)
        return
    }
    guard !elevated && geteuid() != 0 else {
        throw fail("Update archives must be prepared without administrator privileges.")
    }

    try verifyArchive(archive, expectedSHA256: sha256)
    try withUpdateDirectory(archive, format: format) { directory in
        let source = try updatedApp(in: directory, bundleID: bundleID)
        guard try bundleValue(source, "CFBundleShortVersionString") == version else {
            throw fail("The downloaded app version does not match the signed update manifest.")
        }
        try validateSignature(source, expectedTeam: expectedTeam)
        if !isWritable(app.deletingLastPathComponent()) {
            let helper = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
            try elevate(helper: helper, app: app, archive: archive, format: format,
                        sha256: sha256, source: source, version: version, platform: platform,
                        signature: signature, language: language, uid: uid)
        } else {
            try installPreparedApp(source, app: app, bundleID: bundleID, expectedTeam: expectedTeam,
                                   version: version, uid: uid, elevated: false)
        }
    }
}

func showFailure(_ message: String, app: URL, language: String, uid: uid_t, elevated: Bool, restart: Bool = true) {
    let summary = message.split(whereSeparator: \.isNewline).joined(separator: " ").prefix(500)
    let failure = helperText(2, app: app, language: language)
        .replacingOccurrences(of: "{detail}", with: String(summary))
    let script = "display dialog \"\(appleScriptQuote(failure))\" buttons {\"OK\"} default button 1 with title \"RustZIP\""
    if elevated {
        _ = try? run("/bin/launchctl", ["asuser", String(uid), "/usr/bin/osascript", "-e", script])
        if restart { try? openInstalledApp(app, uid: uid, elevated: true) }
    } else {
        _ = try? run("/usr/bin/osascript", ["-e", script])
        if restart { try? openInstalledApp(app, uid: uid, elevated: false) }
    }
}

func recoveryMode() throws {
    guard let appPath = argument("--app"), let pid = argument("--pid").flatMap(Int32.init) else { throw fail("Invalid update recovery arguments.") }
    let app = URL(fileURLWithPath: appPath).standardizedFileURL.resolvingSymlinksInPath()
    let elevated = CommandLine.arguments.contains("--elevated")
    let uid = argument("--uid").flatMap(UInt32.init) ?? getuid()
    guard uid != 0 else { throw fail("RustZIP must restart as a normal user.") }
    #if RUSTZIP_UPDATER_TESTING
    let expectedID = "com.yuhuotech.rustzip.updater-smoke"
    #else
    let expectedID = "com.yuhuotech.rustzip"
    #endif
    guard try bundleValue(app, "CFBundleIdentifier") == expectedID else { throw fail("The recovery target is not RustZIP.") }
    let team = try signingTeam(app)
    try validateSignature(app, expectedTeam: team)
    guard FileManager.default.fileExists(atPath: journalURL(app).path) else { return }
    let journal = try readJournal(app)
    if processStart(journal.updaterPID) == journal.updaterStart { return }
    try waitForExit(pid, started: argument("--pid-start").flatMap(UInt64.init))
    if !isWritable(app.deletingLastPathComponent()) && !elevated {
        let helper = app.appendingPathComponent("Contents/Helpers/RustZIPUpdateHelper")
        let command = [helper.path, "--recover", "--elevated", "--app", app.path, "--pid", "0", "--uid", String(uid)].map(shellQuote).joined(separator: " ")
        try run("/usr/bin/osascript", ["-e", "do shell script \"\(appleScriptQuote(command))\" with administrator privileges"])
    } else {
        let lock = try UpdateLock(app: app)
        defer { withExtendedLifetime(lock) {} }
        guard runningApps(app).isEmpty else { throw fail("Close RustZIP before recovering an interrupted update.") }
        try recoverTransaction(app, expectedTeam: team)
        try openInstalledApp(app, uid: uid, elevated: elevated)
    }
}

func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else {
        return nil
    }
    return CommandLine.arguments[index + 1]
}

do {
    if CommandLine.arguments.contains("--recover") {
        try recoveryMode()
        exit(EXIT_SUCCESS)
    }
    guard
        let appPath = argument("--app"),
        let archivePath = argument("--archive"),
        let format = argument("--format"),
        let sha256 = argument("--sha256"),
        let platform = argument("--platform"),
        let signature = argument("--signature"),
        let version = argument("--version"),
        let language = argument("--language"),
        let pidText = argument("--pid"),
        let pidValue = Int32(pidText)
    else {
        throw fail("The update helper received invalid arguments.")
    }
    let elevated = CommandLine.arguments.contains("--elevated")
    let uid = argument("--uid").flatMap(UInt32.init) ?? getuid()
    let app = URL(fileURLWithPath: appPath)
    guard uid != 0 else { throw fail("RustZIP must restart as a normal user.") }
    let archive = URL(fileURLWithPath: archivePath)
    let preparedApp = argument("--prepared-app").map { URL(fileURLWithPath: $0) }
    let downloadDirectory = archive.deletingLastPathComponent()
    defer { if !elevated { try? FileManager.default.removeItem(at: downloadDirectory) } }
    if !elevated {
        var controller: UpdateSocket?
        if let endpoint = argument("--controller"), let token = argument("--token") {
            controller = try controllerSocket(endpoint, token: token)
        }
        do {
            try verifyReleaseSignature(version: version, platform: platform, format: format, sha256: sha256, signature: signature)
            try verifyArchive(archive, expectedSHA256: sha256)
            try controller?.write(["type": "Ready"])
            if let controller {
                guard try controller.read()["type"] as? String == "Armed" else { throw fail("RustZIP cancelled the update handoff.") }
                try controller.write(["type": "Armed"])
            }
        } catch {
            try? controller?.write(["type": "Failed", "detail": error.localizedDescription])
            throw error
        }
        try waitForExit(pid_t(pidValue), started: argument("--pid-start").flatMap(UInt64.init))
    }
    do {
        try install(app: app, archive: archive, format: format, sha256: sha256,
                    preparedApp: preparedApp, version: version, platform: platform, signature: signature,
                    language: language, uid: uid, elevated: elevated)
    } catch {
        #if !RUSTZIP_UPDATER_TESTING
        if !elevated {
            showFailure(error.localizedDescription, app: app, language: language, uid: uid, elevated: false, restart: !FileManager.default.fileExists(atPath: journalURL(app).path))
        }
        #endif
        throw error
    }
} catch {
    if CommandLine.arguments.contains("--recover"), let path = argument("--app") {
        showFailure(error.localizedDescription, app: URL(fileURLWithPath: path), language: "en-US", uid: argument("--uid").flatMap(UInt32.init) ?? getuid(), elevated: CommandLine.arguments.contains("--elevated"), restart: false)
    }
    fputs("RustZIP update failed.\n", stderr)
    exit(EXIT_FAILURE)
}
