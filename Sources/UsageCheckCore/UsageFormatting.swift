import Foundation

public enum UsageFormatting {
    public static func tokenCount(_ tokens: Int64) -> String {
        let absValue = abs(Double(tokens))
        let sign = tokens < 0 ? "-" : ""

        if absValue >= 1_000_000_000 {
            return "\(sign)\(trim(absValue / 1_000_000_000))B tok"
        }

        if absValue >= 1_000_000 {
            return "\(sign)\(trim(absValue / 1_000_000))M tok"
        }

        if absValue >= 1_000 {
            return "\(sign)\(trim(absValue / 1_000))K tok"
        }

        return "\(tokens) tok"
    }

    public static func percent(_ value: Double) -> String {
        "\(trim(value))%"
    }

    public static func relativeReset(_ date: Date?, now: Date = Date()) -> String? {
        guard let date else {
            return nil
        }

        let seconds = max(0, Int(date.timeIntervalSince(now)))
        let days = seconds / 86_400
        let hours = (seconds % 86_400) / 3_600
        let minutes = (seconds % 3_600) / 60

        if days > 0 {
            return "\(days)d \(hours)h reset"
        }

        if hours > 0 {
            return "\(hours)h \(minutes)m reset"
        }

        return "\(minutes)m reset"
    }

    public static func timestamp(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.timeStyle = .medium
        formatter.dateStyle = .none
        return formatter.string(from: date)
    }

    public static func line(for pool: PoolUsage, window: UsageWindow, now: Date = Date()) -> String {
        let tokens = tokenCount(pool.totals[window])

        let quota: QuotaUsage?
        switch window {
        case .fiveHours:
            quota = pool.fiveHourQuota
        case .week:
            quota = pool.weekQuota
        case .month:
            quota = nil
        }

        guard let quota else {
            return "\(window.title): \(tokens)"
        }

        if let reset = relativeReset(quota.resetsAt, now: now) {
            return "\(window.title): \(percent(quota.percent)) · \(tokens) · \(reset)"
        }

        return "\(window.title): \(percent(quota.percent)) · \(tokens)"
    }

    private static func trim(_ value: Double) -> String {
        let rounded = (value * 10).rounded() / 10
        if rounded.rounded() == rounded {
            return String(Int(rounded))
        }

        return String(format: "%.1f", rounded)
    }
}
