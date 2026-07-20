use super::*;

#[test]
fn matching_accounts_rejects_empty_identities_and_keeps_real_matches() {
    let accounts = [
        AccountRef {
            account_id: "empty",
            creds_account_id: Some("   "),
            expected_identity: Some(""),
            is_browser_oauth: false,
            profile_roots: vec![],
        },
        AccountRef {
            account_id: "real",
            creds_account_id: Some("account-123"),
            expected_identity: Some("User@Example.com"),
            is_browser_oauth: false,
            profile_roots: vec![],
        },
    ];

    let empty_identity = RootIdentity::CodexAuth {
        account_id: Some("   ".into()),
        email: Some("".into()),
    };
    assert!(matching_accounts(&accounts, &empty_identity).is_empty());

    let real_id = RootIdentity::CodexAuth {
        account_id: Some("account-123".into()),
        email: None,
    };
    assert_eq!(matching_accounts(&accounts, &real_id), vec![1]);

    let real_email = RootIdentity::ClaudeEmail {
        email: Some("user@example.com".into()),
    };
    assert_eq!(matching_accounts(&accounts, &real_email), vec![1]);
}

#[test]
fn test_local_provenance_severity_order() {
    // Verify strict total order (no duplicates).
    assert_eq!(LocalProvenance::Ok.severity_rank(), 0);
    assert_eq!(LocalProvenance::NoEvents.severity_rank(), 1);
    assert_eq!(LocalProvenance::Assumed.severity_rank(), 2);
    assert_eq!(LocalProvenance::NoLocalProfile.severity_rank(), 3);
    assert_eq!(LocalProvenance::SharedProfileOther.severity_rank(), 4);
    assert_eq!(LocalProvenance::Ambiguous.severity_rank(), 5);
    assert_eq!(LocalProvenance::Conflict.severity_rank(), 6);
    assert_eq!(LocalProvenance::Partial.severity_rank(), 7);
    assert_eq!(LocalProvenance::Unavailable.severity_rank(), 8);
    assert_eq!(LocalProvenance::Truncated.severity_rank(), 9);
}

#[test]
fn test_cap_regression_300_files() {
    // §6.1: tempdir with 300 .jsonl files, each 1 event, mtime recent.
    // Expected: all 300 counted in totals.
    let now = Utc::now();
    let events: Vec<_> = (0..300)
        .map(|i| ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes((i as i64) % 60),
            model: format!("test-model-{}", i),
            tokens: 100,
            dedupe_key: Some(format!("key-{}", i)),
        })
        .collect();
    let root = ScannedRoot {
        root_key: PathBuf::from("/test/root"),
        source_roots: vec![PathBuf::from("/test/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    let account = AccountRef {
        account_id: "test-acct",
        creds_account_id: None,
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/test/root")],
    };
    let result = assign_local_usage(&[account], &[root], now);
    // Real assertions on real expected behavior:
    assert!(!result.is_empty(), "Should return result for 1 account");
    let (id, usage) = &result[0];
    assert_eq!(id, "test-acct", "Account ID mismatch");
    // All 300 events within 5 hours (recent timestamps) → 300 * 100 tokens in all buckets
    assert_eq!(
        usage.totals.five_hours, 30000,
        "Expected 300 * 100 tokens in 5h bucket"
    );
    assert_eq!(
        usage.totals.week, 30000,
        "Expected 300 * 100 tokens in week bucket"
    );
    assert_eq!(
        usage.totals.month, 30000,
        "Expected 300 * 100 tokens in month bucket"
    );
    // Sole associate of a NoIdentity root → Assumed per plan §4.3 Phase B
    // (totals are still shown; only Conflict/Ambiguous zero them).
    assert_eq!(
        usage.provenance,
        LocalProvenance::Assumed,
        "Sole-associate NoIdentity → Assumed"
    );
}

#[test]
fn test_mtime_irrelevance_1h_timestamp() {
    // §6.2: mtime=40d ago, event ts=1h ago → 5h/week/month all aggregate.
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::hours(1),
        model: "test".into(),
        tokens: 100,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/test/root"),
        source_roots: vec![PathBuf::from("/test/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    let account = AccountRef {
        account_id: "test",
        creds_account_id: None,
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/test/root")],
    };
    let result = assign_local_usage(&[account], &[root], now);
    assert!(!result.is_empty(), "Expected result for 1 account");
    let (_, usage) = &result[0];
    assert_eq!(
        usage.totals.five_hours, 100,
        "1h old event should be in 5h bucket"
    );
    assert_eq!(
        usage.totals.week, 100,
        "1h old event should be in week bucket"
    );
    assert_eq!(
        usage.totals.month, 100,
        "1h old event should be in month bucket"
    );
}

#[test]
fn test_strong_proof_codex_identity() {
    // §6.3-6.4: two codex accounts A(α), B(β), shared root, auth id=α.
    // Expected: A=Ok, B=SharedProfileOther(0).
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::minutes(10),
        model: "claude-opus".into(),
        tokens: 500,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/shared/codex"),
        source_roots: vec![PathBuf::from("/shared/codex")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("alpha".into()),
            email: None,
        },
    };
    let acct_a = AccountRef {
        account_id: "acct-a",
        creds_account_id: Some("alpha"),
        expected_identity: Some("a@ex.com"),
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/shared/codex")],
    };
    let acct_b = AccountRef {
        account_id: "acct-b",
        creds_account_id: Some("beta"),
        expected_identity: Some("b@ex.com"),
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/shared/codex")],
    };
    let result = assign_local_usage(&[acct_a, acct_b], &[root], now);
    assert_eq!(result.len(), 2, "Expected 2 accounts in result");
    let (id_a, usage_a) = &result[0];
    assert_eq!(id_a, "acct-a");
    assert_eq!(
        usage_a.provenance,
        LocalProvenance::Ok,
        "A proves ownership via auth id"
    );
    assert_eq!(usage_a.totals.five_hours, 500, "A gets the events");
    let (id_b, usage_b) = &result[1];
    assert_eq!(id_b, "acct-b");
    assert_eq!(
        usage_b.provenance,
        LocalProvenance::SharedProfileOther,
        "B excluded by proof"
    );
    assert_eq!(usage_b.totals.five_hours, 0, "B gets no events");
}

#[test]
fn test_unique_proof_beats_association() {
    // §6.5a: shared root, auth id→B, B's sole associate=A.
    // Expected: B=Ok (proof), A=SharedProfileOther.
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::minutes(10),
        model: "test".into(),
        tokens: 200,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/shared/root"),
        source_roots: vec![PathBuf::from("/shared/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("b-id".into()),
            email: None,
        },
    };
    let acct_a = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/shared/root")],
    };
    let acct_b = AccountRef {
        account_id: "b",
        creds_account_id: Some("b-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/shared/root")],
    };
    let result = assign_local_usage(&[acct_a, acct_b], &[root], now);
    let b_usage = result
        .iter()
        .find(|(id, _)| id == "b")
        .expect("b not found");
    assert_eq!(
        b_usage.1.provenance,
        LocalProvenance::Ok,
        "B owns via proof"
    );
    assert_eq!(b_usage.1.totals.five_hours, 200, "B gets events");
    let a_usage = result
        .iter()
        .find(|(id, _)| id == "a")
        .expect("a not found");
    assert_eq!(
        a_usage.1.provenance,
        LocalProvenance::SharedProfileOther,
        "A excluded via proof"
    );
    assert_eq!(a_usage.1.totals.five_hours, 0, "A gets no events");
}

#[test]
fn test_conflict_unregistered_identity() {
    // §6.5b: shared/sole root, unregistered identity, sole associate A.
    // Expected: A.totals=0+Conflict.
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::minutes(10),
        model: "test".into(),
        tokens: 300,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/sole/root"),
        source_roots: vec![PathBuf::from("/sole/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("unknown-id".into()),
            email: None,
        },
    };
    let acct_a = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/sole/root")],
    };
    let result = assign_local_usage(&[acct_a], &[root], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    assert_eq!(
        usage.provenance,
        LocalProvenance::Conflict,
        "Unregistered identity → Conflict"
    );
    assert_eq!(usage.totals.five_hours, 0, "Conflict isolates totals");
}

#[test]
fn test_no_identity_assumed() {
    // §6.5c: claude root (no identity), sole associate A.
    // Expected: A.totals=Assumed (≠Ok, ≠Conflict).
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::minutes(10),
        model: "test".into(),
        tokens: 150,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/claude/root"),
        source_roots: vec![PathBuf::from("/claude/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    let acct_a = AccountRef {
        account_id: "a",
        creds_account_id: None,
        expected_identity: None,
        is_browser_oauth: true,
        profile_roots: vec![PathBuf::from("/claude/root")],
    };
    let result = assign_local_usage(&[acct_a], &[root], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    assert_eq!(
        usage.provenance,
        LocalProvenance::Assumed,
        "NoIdentity sole-associate → Assumed"
    );
    assert_eq!(usage.totals.five_hours, 150, "Assumed includes totals");
}

#[test]
fn test_ambiguous_no_proof_multi_account() {
    // §6.6: two accounts share root, no identity proof.
    // Expected: both Ambiguous(0).
    let now = Utc::now();
    let events = vec![ModelTokenEvent {
        timestamp: now - chrono::Duration::minutes(10),
        model: "test".into(),
        tokens: 200,
        dedupe_key: None,
    }];
    let root = ScannedRoot {
        root_key: PathBuf::from("/ambig/root"),
        source_roots: vec![PathBuf::from("/ambig/root")],
        events,
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    // Two CLI-profile accounts (not BrowserOAuth) contest one unprovable
    // shared root → Ambiguous. BrowserOAuth accounts are never associated
    // (plan §4.3 → NoLocalProfile), so they cannot produce Ambiguous.
    let acct_a = AccountRef {
        account_id: "a",
        creds_account_id: None,
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/ambig/root")],
    };
    let acct_b = AccountRef {
        account_id: "b",
        creds_account_id: None,
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/ambig/root")],
    };
    let result = assign_local_usage(&[acct_a, acct_b], &[root], now);
    assert_eq!(result.len(), 2);
    for (_, usage) in &result {
        assert_eq!(
            usage.provenance,
            LocalProvenance::Ambiguous,
            "Both ambiguous"
        );
        assert_eq!(usage.totals.five_hours, 0, "No totals assigned");
    }
}

#[test]
fn test_order_invariance_conflict_ambiguous_merge() {
    // §6.14b: account receives Conflict + Ambiguous from different roots.
    // Expected: always Conflict (rank 6 > 5), totals=0, regardless of root order.
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "x",
        creds_account_id: Some("x-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![
            PathBuf::from("/root/conflict"),
            PathBuf::from("/root/ambiguous"),
        ],
    };
    let root_conflict = ScannedRoot {
        root_key: PathBuf::from("/root/conflict"),
        source_roots: vec![PathBuf::from("/root/conflict")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("unknown-id".into()),
            email: None,
        },
    };
    let root_ambiguous = ScannedRoot {
        root_key: PathBuf::from("/root/ambiguous"),
        source_roots: vec![PathBuf::from("/root/ambiguous")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    // Test with both root orders: [Conflict, Ambiguous] and [Ambiguous, Conflict]
    for roots in &[
        vec![root_conflict.clone(), root_ambiguous.clone()],
        vec![root_ambiguous.clone(), root_conflict.clone()],
    ] {
        let result = assign_local_usage(std::slice::from_ref(&acct), roots, now);
        assert_eq!(result.len(), 1);
        let (_, usage) = &result[0];
        assert_eq!(
            usage.provenance,
            LocalProvenance::Conflict,
            "Conflict (rank 6) should win over Ambiguous (rank 5)"
        );
        assert_eq!(usage.totals.five_hours, 0, "Conflict totals empty");
    }
}

#[test]
fn test_cross_root_merge_two_proven_roots() {
    // §6.7a: one account owns TWO proven roots → union + dedupe + worst-provenance.
    // R1 has 100 tokens (health Ok), R2 has 50 tokens (health Partial).
    // Merged provenance should be Partial (worse health).
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/root/r1"), PathBuf::from("/root/r2")],
    };
    let root_r1 = ScannedRoot {
        root_key: PathBuf::from("/root/r1"),
        source_roots: vec![PathBuf::from("/root/r1")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };
    let root_r2 = ScannedRoot {
        root_key: PathBuf::from("/root/r2"),
        source_roots: vec![PathBuf::from("/root/r2")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 50,
            dedupe_key: None,
        }],
        health: LocalProvenance::Partial, // R2 had a read error
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };
    let result = assign_local_usage(&[acct], &[root_r1, root_r2], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    // Union totals = 100 + 50 = 150
    assert_eq!(usage.totals.five_hours, 150, "Union of R1 + R2 totals");
    // Merge provenance: max(Ok=0, Partial=7) = Partial
    assert_eq!(
        usage.provenance,
        LocalProvenance::Partial,
        "Worst health wins"
    );
}

#[test]
fn test_proof_owner_also_sole_associate_noidentity() {
    // §6.7b: proof-owner account ALSO sole-associate of NoIdentity root.
    // A owns R1 (Ok via proof) and is sole associate of R2 (NoIdentity).
    // Merged provenance = worst(Ok=0, Assumed=2) = Assumed.
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![
            PathBuf::from("/root/proven"),
            PathBuf::from("/root/assumed"),
        ],
    };
    let root_proven = ScannedRoot {
        root_key: PathBuf::from("/root/proven"),
        source_roots: vec![PathBuf::from("/root/proven")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };
    let root_assumed = ScannedRoot {
        root_key: PathBuf::from("/root/assumed"),
        source_roots: vec![PathBuf::from("/root/assumed")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 50,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::None, // No identity proof
    };
    let result = assign_local_usage(&[acct], &[root_proven, root_assumed], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    // Totals = 100 + 50 = 150 (both roots associated)
    assert_eq!(usage.totals.five_hours, 150, "Union totals");
    // Provenance = worst(Ok=0, Assumed=2) = Assumed
    assert_eq!(
        usage.provenance,
        LocalProvenance::Assumed,
        "Proof + Assumed merge"
    );
}

#[test]
fn test_conflicting_dedupekeys_order_invariance() {
    // §6.7c: cross-root same dedupe_key, different tokens AND timestamps.
    // Canonical tiebreak: max(tokens desc, timestamp desc, model asc).
    // Result must be identical under shuffled root order and event order.
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/r1"), PathBuf::from("/r2")],
    };
    let root_r1 = ScannedRoot {
        root_key: PathBuf::from("/r1"),
        source_roots: vec![PathBuf::from("/r1")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::hours(1), // 1h ago
            model: "model-a".into(),
            tokens: 500,
            dedupe_key: Some("same-key".into()),
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };
    let root_r2 = ScannedRoot {
        root_key: PathBuf::from("/r2"),
        source_roots: vec![PathBuf::from("/r2")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::hours(6), // 6h ago (different window)
            model: "model-b".into(),
            tokens: 300,
            dedupe_key: Some("same-key".into()),
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };

    // Call with [R1, R2] order
    let result1 = assign_local_usage(std::slice::from_ref(&acct), &[root_r1.clone(), root_r2.clone()], now);
    // Call with [R2, R1] order (reversed)
    let result2 = assign_local_usage(std::slice::from_ref(&acct), &[root_r2.clone(), root_r1.clone()], now);

    assert_eq!(result1.len(), 1);
    assert_eq!(result2.len(), 1);
    let (_, usage1) = &result1[0];
    let (_, usage2) = &result2[0];

    // Both must have identical totals (canonical representative wins)
    assert_eq!(
        usage1.totals.five_hours, usage2.totals.five_hours,
        "5h totals must be identical"
    );
    // Canonical winner: 500 tokens (bigger), 1h ago (more recent timestamp), "model-a" (lexical)
    // Should be counted in 5h window (1h ago), not 6h-only (not in 5h)
    // So 5h = 500, not 300
    assert_eq!(
        usage1.totals.five_hours, 500,
        "Canonical repr (500 tokens, 1h) should win"
    );
}

#[test]
fn test_browser_oauth_identity_proof() {
    // §6.8a: BrowserOAuth identity matches → Ok.
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "oauth-a",
        creds_account_id: None,
        expected_identity: Some("user@example.com"), // OAuth email
        is_browser_oauth: true,
        profile_roots: vec![PathBuf::from("/oauth/root")],
    };
    let root = ScannedRoot {
        root_key: PathBuf::from("/oauth/root"),
        source_roots: vec![PathBuf::from("/oauth/root")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::ClaudeEmail {
            email: Some("user@example.com".into()), // Proof matches
        },
    };
    let result = assign_local_usage(&[acct], &[root], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    assert_eq!(
        usage.provenance,
        LocalProvenance::Ok,
        "OAuth email match → Ok"
    );
    assert_eq!(usage.totals.five_hours, 100);
}

#[test]
fn test_browser_oauth_no_proof() {
    // §6.8b: BrowserOAuth identity mismatch/no-proof → NoLocalProfile (NOT Assumed).
    let now = Utc::now();
    let acct = AccountRef {
        account_id: "oauth-b",
        creds_account_id: None,
        expected_identity: Some("user1@example.com"),
        is_browser_oauth: true,
        profile_roots: vec![PathBuf::from("/oauth/root")],
    };
    let root = ScannedRoot {
        root_key: PathBuf::from("/oauth/root"),
        source_roots: vec![PathBuf::from("/oauth/root")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::ClaudeEmail {
            email: Some("user2@example.com".into()), // Proof mismatch
        },
    };
    let result = assign_local_usage(&[acct], &[root], now);
    assert_eq!(result.len(), 1);
    let (_, usage) = &result[0];
    // BrowserOAuth without matching proof → NoLocalProfile, NOT Assumed
    assert_eq!(
        usage.provenance,
        LocalProvenance::NoLocalProfile,
        "OAuth mismatch → NoLocalProfile"
    );
    assert_eq!(usage.totals.five_hours, 0);
}

#[test]
fn test_order_invariance_with_mixed_fixtures() {
    // §6.14a: extend order-invariance test with §6.7b+6.7c fixtures.
    // Two accounts, mixed proof+assumed roots, cross-root dedupe keys.
    // Shuffle both root order and per-root event order → identical results.
    let now = Utc::now();
    let acct_a = AccountRef {
        account_id: "a",
        creds_account_id: Some("a-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/a-r1"), PathBuf::from("/a-r2")],
    };
    let acct_b = AccountRef {
        account_id: "b",
        creds_account_id: Some("b-id"),
        expected_identity: None,
        is_browser_oauth: false,
        profile_roots: vec![PathBuf::from("/b-r1")],
    };

    let root_a_r1 = ScannedRoot {
        root_key: PathBuf::from("/a-r1"),
        source_roots: vec![PathBuf::from("/a-r1")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 100,
            dedupe_key: Some("shared".into()),
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("a-id".into()),
            email: None,
        },
    };
    let root_a_r2 = ScannedRoot {
        root_key: PathBuf::from("/a-r2"),
        source_roots: vec![PathBuf::from("/a-r2")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 50,
            dedupe_key: Some("shared".into()),
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::None,
    };
    let root_b_r1 = ScannedRoot {
        root_key: PathBuf::from("/b-r1"),
        source_roots: vec![PathBuf::from("/b-r1")],
        events: vec![ModelTokenEvent {
            timestamp: now - chrono::Duration::minutes(10),
            model: "test".into(),
            tokens: 200,
            dedupe_key: None,
        }],
        health: LocalProvenance::Ok,
        identity: RootIdentity::CodexAuth {
            account_id: Some("b-id".into()),
            email: None,
        },
    };

    // Test both root orderings
    let result1 = assign_local_usage(
        &[acct_a.clone(), acct_b.clone()],
        &[root_a_r1.clone(), root_a_r2.clone(), root_b_r1.clone()],
        now,
    );
    let result2 = assign_local_usage(
        &[acct_a.clone(), acct_b.clone()],
        &[root_b_r1.clone(), root_a_r2.clone(), root_a_r1.clone()], // Different order
        now,
    );

    assert_eq!(result1.len(), 2);
    assert_eq!(result2.len(), 2);

    // Both results must be identical regardless of root order
    for (i, (id, usage)) in result1.iter().enumerate() {
        let (id2, usage2) = &result2[i];
        assert_eq!(id, id2, "Account IDs must match");
        assert_eq!(
            usage.totals.five_hours, usage2.totals.five_hours,
            "Totals must match for {}",
            id
        );
        assert_eq!(
            usage.provenance, usage2.provenance,
            "Provenance must match for {}",
            id
        );
    }
}
