import Foundation

@main
enum QuickLookSmoke {
    static func main() throws {
        guard CommandLine.arguments.count == 8 else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 1)
        }
        let normal = try ZipIndex.read(URL(fileURLWithPath: CommandLine.arguments[1]))
        guard normal.totalEntries == 3,
              normal.entries.count == 3,
              normal.entries.reduce(UInt64(0), { $0 + $1.size }) == normal.totalUncompressedSize,
              normal.entries.map(\.name).contains("資料/數據.csv"),
              normal.entries.map(\.name).contains("unsafe <tag> & name.txt")
        else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 2)
        }

        let zip64 = try ZipIndex.read(URL(fileURLWithPath: CommandLine.arguments[2]))
        guard zip64.totalEntries == normal.totalEntries else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 4)
        }

        let encrypted = try ZipIndex.read(URL(fileURLWithPath: CommandLine.arguments[7]))
        guard encrypted.entries.first?.encrypted == true,
              encrypted.entries.dropFirst().allSatisfy({ !$0.encrypted })
        else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 5)
        }

        for (path, expected) in [
            (CommandLine.arguments[3], PreviewFailure.invalidArchive),
            (CommandLine.arguments[4], PreviewFailure.metadataLimit),
            (CommandLine.arguments[5], PreviewFailure.unsupportedArchive),
            (CommandLine.arguments[6], PreviewFailure.unsupportedArchive)
        ] {
            do {
                _ = try ZipIndex.read(URL(fileURLWithPath: path))
                throw NSError(domain: "RustZIPQuickLookSmoke", code: 6)
            } catch let actual as PreviewFailure {
                guard String(describing: actual) == String(describing: expected) else {
                    throw NSError(domain: "RustZIPQuickLookSmoke", code: 7)
                }
            }
        }
        print("Quick Look ZIP index smoke passed")
    }
}
