import Foundation

@main
enum QuickLookSmoke {
    static func main() throws {
        guard CommandLine.arguments.count == 7 else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 1)
        }
        let normal = try ZipIndex.read(URL(fileURLWithPath: CommandLine.arguments[1]))
        guard normal.totalEntries == 3,
              normal.entries.map(\.name).contains("資料/數據.csv"),
              normal.entries.map(\.name).contains("unsafe <tag> & name.txt")
        else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 2)
        }
        let html = render(normal, title: "unsafe <archive>.zip")
        guard html.contains("unsafe &lt;archive&gt;.zip"),
              html.contains("unsafe &lt;tag&gt; &amp; name.txt"),
              !html.contains("<script>")
        else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 3)
        }

        let zip64 = try ZipIndex.read(URL(fileURLWithPath: CommandLine.arguments[2]))
        guard zip64.totalEntries == normal.totalEntries else {
            throw NSError(domain: "RustZIPQuickLookSmoke", code: 4)
        }

        for (path, expected) in [
            (CommandLine.arguments[3], PreviewFailure.invalidArchive),
            (CommandLine.arguments[4], PreviewFailure.metadataLimit),
            (CommandLine.arguments[5], PreviewFailure.unsupportedArchive),
            (CommandLine.arguments[6], PreviewFailure.unsupportedArchive)
        ] {
            do {
                _ = try ZipIndex.read(URL(fileURLWithPath: path))
                throw NSError(domain: "RustZIPQuickLookSmoke", code: 5)
            } catch let actual as PreviewFailure {
                guard String(describing: actual) == String(describing: expected) else {
                    throw NSError(domain: "RustZIPQuickLookSmoke", code: 6)
                }
            }
        }
        print("Quick Look ZIP index smoke passed")
    }
}
