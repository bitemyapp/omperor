// Migrate only old local production-app pins; never add a pin or touch Dev.
import Foundation

let arguments = CommandLine.arguments
 guard arguments.count == 3 else {
    fputs("usage: swift update-installed-dock.swift <legacy.app> <installed.app>\n", stderr)
    exit(2)
}
let legacy = URL(fileURLWithPath: arguments[1]).standardizedFileURL
let installed = URL(fileURLWithPath: arguments[2]).standardizedFileURL
let domain = "com.apple.dock" as CFString
let key = "persistent-apps" as CFString
 guard var tiles = CFPreferencesCopyAppValue(key, domain) as? [[String: Any]] else {
    exit(0)
}
var changed = false
for index in tiles.indices {
    guard var data = tiles[index]["tile-data"] as? [String: Any],
          let file = data["file-data"] as? [String: Any],
          let raw = file["_CFURLString"] as? String,
          let url = URL(string: raw), url.isFileURL,
          url.standardizedFileURL == legacy else { continue }
    // Bookmarks resolve by file identity and would keep selecting the old app.
    data.removeValue(forKey: "book")
    data["file-data"] = ["_CFURLString": installed.absoluteString, "_CFURLStringType": 15]
    data["file-label"] = "Omperor"
    data["bundle-identifier"] = "com.bitemyapp.omperor"
    tiles[index]["tile-data"] = data
    changed = true
}
if changed {
    CFPreferencesSetAppValue(key, tiles as CFArray, domain)
    guard CFPreferencesAppSynchronize(domain) else {
        fputs("Could not save the updated Omperor Dock shortcut\n", stderr)
        exit(1)
    }
    print("Updated pinned Omperor shortcut to \(installed.path)")
    // Restart the Dock, not any application. It reloads the saved pin in place.
    let reload = Process()
    reload.executableURL = URL(fileURLWithPath: "/usr/bin/killall")
    reload.arguments = ["Dock"]
    try reload.run()
    reload.waitUntilExit()
}
