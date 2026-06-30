import AppKit
import UsageCheckCore

@main
struct UsageCheckMain {
    static func main() {
        let application = NSApplication.shared
        let delegate = AppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) {
            application.run()
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private var snapshot: UsageSnapshot?
    private var refreshTimer: Timer?
    private var isRefreshing = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        configureStatusItem()
        rebuildMenu()
        refreshNow()

        refreshTimer = Timer.scheduledTimer(withTimeInterval: 5 * 60, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.refreshNow()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        refreshTimer?.invalidate()
    }

    private func configureStatusItem() {
        guard let button = statusItem.button else {
            return
        }

        if let image = NSImage(systemSymbolName: "chart.bar.xaxis", accessibilityDescription: "UsageCheck") {
            image.isTemplate = true
            button.image = image
        } else {
            button.title = "UC"
        }

        button.toolTip = "UsageCheck"
    }

    @objc private func refreshMenuItemSelected() {
        refreshNow()
    }

    @objc private func quitSelected() {
        NSApplication.shared.terminate(nil)
    }

    private func refreshNow() {
        guard !isRefreshing else {
            return
        }

        isRefreshing = true
        rebuildMenu()

        Task {
            let snapshot = await Task.detached(priority: .utility) {
                await UsageReader().readSnapshot()
            }.value

            await MainActor.run {
                self.snapshot = snapshot
                self.isRefreshing = false
                self.rebuildMenu()
            }
        }
    }

    private func rebuildMenu() {
        let menu = NSMenu()

        let title = snapshot.map { "UsageCheck · \(UsageFormatting.timestamp($0.capturedAt))" } ?? "UsageCheck"
        let titleItem = disabledItem(title)
        titleItem.attributedTitle = NSAttributedString(
            string: title,
            attributes: [.font: NSFont.boldSystemFont(ofSize: NSFont.systemFontSize)]
        )
        menu.addItem(titleItem)

        if isRefreshing {
            menu.addItem(disabledItem("Refreshing..."))
        }

        menu.addItem(.separator())

        if let snapshot {
            for provider in snapshot.providers {
                append(provider: provider, to: menu, now: snapshot.capturedAt)
            }
        } else {
            menu.addItem(disabledItem("Loading usage data..."))
        }

        menu.addItem(.separator())
        let refresh = NSMenuItem(title: "Refresh Now", action: #selector(refreshMenuItemSelected), keyEquivalent: "r")
        refresh.target = self
        refresh.isEnabled = !isRefreshing
        menu.addItem(refresh)

        let quit = NSMenuItem(title: "Quit UsageCheck", action: #selector(quitSelected), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)

        statusItem.menu = menu
    }

    private func append(provider: ProviderUsage, to menu: NSMenu, now: Date) {
        let heading = disabledItem("\(provider.provider.displayName) · \(provider.sourceSummary)")
        heading.attributedTitle = NSAttributedString(
            string: heading.title,
            attributes: [.font: NSFont.boldSystemFont(ofSize: NSFont.systemFontSize)]
        )
        menu.addItem(heading)

        for pool in provider.pools {
            menu.addItem(disabledItem("  \(pool.displayName)"))
            menu.addItem(disabledItem("    \(UsageFormatting.line(for: pool, window: .fiveHours, now: now))"))
            menu.addItem(disabledItem("    \(UsageFormatting.line(for: pool, window: .week, now: now))"))
            menu.addItem(disabledItem("    \(UsageFormatting.line(for: pool, window: .month, now: now))"))
            if let note = pool.note {
                menu.addItem(disabledItem("    \(note)"))
            }
        }

        menu.addItem(.separator())
    }

    private func disabledItem(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }
}
