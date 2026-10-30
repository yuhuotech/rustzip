import AppKit
import FinderSync

final class FinderSync: FIFinderSync {
    private let controller = FIFinderSyncController.default()

    override init() {
        super.init()
        controller.directoryURLs = [URL(fileURLWithPath: "/")]
    }

    override func menu(for menu: FIMenuKind) -> NSMenu? {
        guard menu == .contextualMenuForItems,
              let selected = controller.selectedItemURLs(),
              !selected.isEmpty
        else {
            return nil
        }

        let result = NSMenu()
        result.addItem(menuItem("finder.compress", action: #selector(compressSelection(_:))))
        if selected.allSatisfy(isSupportedArchive) {
            result.addItem(menuItem("finder.extractHere", action: #selector(extractSelection(_:))))
        }
        return result
    }

    @objc private func compressSelection(_ sender: NSMenuItem) {
        dispatch(action: "compress")
    }

    @objc private func extractSelection(_ sender: NSMenuItem) {
        dispatch(action: "extract-here")
    }

    private func menuItem(_ key: String, action: Selector) -> NSMenuItem {
        let item = NSMenuItem(
            title: NSLocalizedString(key, bundle: .main, comment: "Finder action"),
            action: action,
            keyEquivalent: ""
        )
        item.target = self
        return item
    }

    private func isSupportedArchive(_ url: URL) -> Bool {
        var extensions: Set<String> = [
            "zip", "7z", "tar", "gz", "tgz", "xz", "txz", "zst", "tzst",
            "bz2", "tbz2", "lz4", "iso", "cab", "cpio", "ar", "deb", "rpm",
            "lha", "lzh", "xar", "chm"
        ]
        #if RUSTZIP_WITH_RAR
        extensions.insert("rar")
        #endif
        return extensions.contains(url.pathExtension.lowercased())
    }

    private func dispatch(action: String) {
        guard let selected = controller.selectedItemURLs(), !selected.isEmpty else {
            return
        }
        var components = URLComponents()
        components.scheme = "rustzip"
        components.host = action
        components.queryItems = selected.map {
            URLQueryItem(name: "path", value: $0.absoluteString)
        }
        guard let url = components.url, NSWorkspace.shared.open(url) else {
            NSLog("RustZIP Finder action could not be opened")
            return
        }
    }
}
