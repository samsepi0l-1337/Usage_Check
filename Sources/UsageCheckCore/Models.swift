import Foundation

public enum ProviderID: String, CaseIterable, Sendable {
    case codex
    case claude
    case gemini

    public var displayName: String {
        switch self {
        case .codex:
            return "Codex"
        case .claude:
            return "Claude Code"
        case .gemini:
            return "Gemini"
        }
    }
}

public enum UsageWindow: String, CaseIterable, Sendable {
    case fiveHours
    case week
    case month

    public var title: String {
        switch self {
        case .fiveHours:
            return "5h"
        case .week:
            return "7d"
        case .month:
            return "30d"
        }
    }

    public var duration: TimeInterval {
        switch self {
        case .fiveHours:
            return 5 * 60 * 60
        case .week:
            return 7 * 24 * 60 * 60
        case .month:
            return 30 * 24 * 60 * 60
        }
    }
}

public struct WindowTotals: Equatable, Sendable {
    public var fiveHours: Int64
    public var week: Int64
    public var month: Int64

    public init(fiveHours: Int64 = 0, week: Int64 = 0, month: Int64 = 0) {
        self.fiveHours = fiveHours
        self.week = week
        self.month = month
    }

    public subscript(_ window: UsageWindow) -> Int64 {
        switch window {
        case .fiveHours:
            return fiveHours
        case .week:
            return week
        case .month:
            return month
        }
    }

    mutating func add(tokens: Int64, at timestamp: Date, now: Date) {
        guard tokens > 0 else {
            return
        }

        let age = now.timeIntervalSince(timestamp)
        guard age >= 0 else {
            return
        }

        if age <= UsageWindow.month.duration {
            month += tokens
        }

        if age <= UsageWindow.week.duration {
            week += tokens
        }

        if age <= UsageWindow.fiveHours.duration {
            fiveHours += tokens
        }
    }
}

public struct QuotaUsage: Equatable, Sendable {
    public var percent: Double
    public var resetsAt: Date?
    public var windowSeconds: Int?

    public init(percent: Double, resetsAt: Date? = nil, windowSeconds: Int? = nil) {
        self.percent = percent
        self.resetsAt = resetsAt
        self.windowSeconds = windowSeconds
    }
}

public struct PoolUsage: Equatable, Sendable {
    public var id: String
    public var displayName: String
    public var totals: WindowTotals
    public var fiveHourQuota: QuotaUsage?
    public var weekQuota: QuotaUsage?
    public var note: String?

    public init(
        id: String,
        displayName: String,
        totals: WindowTotals,
        fiveHourQuota: QuotaUsage? = nil,
        weekQuota: QuotaUsage? = nil,
        note: String? = nil
    ) {
        self.id = id
        self.displayName = displayName
        self.totals = totals
        self.fiveHourQuota = fiveHourQuota
        self.weekQuota = weekQuota
        self.note = note
    }
}

public struct ProviderUsage: Equatable, Sendable {
    public var provider: ProviderID
    public var pools: [PoolUsage]
    public var sourceSummary: String

    public init(provider: ProviderID, pools: [PoolUsage], sourceSummary: String) {
        self.provider = provider
        self.pools = pools
        self.sourceSummary = sourceSummary
    }
}

public struct UsageSnapshot: Equatable, Sendable {
    public var capturedAt: Date
    public var providers: [ProviderUsage]

    public init(capturedAt: Date, providers: [ProviderUsage]) {
        self.capturedAt = capturedAt
        self.providers = providers
    }
}

public struct ModelTokenEvent: Equatable, Sendable {
    public var timestamp: Date
    public var model: String
    public var tokens: Int64
    public var dedupeKey: String?

    public init(timestamp: Date, model: String, tokens: Int64, dedupeKey: String? = nil) {
        self.timestamp = timestamp
        self.model = model
        self.tokens = tokens
        self.dedupeKey = dedupeKey
    }
}
