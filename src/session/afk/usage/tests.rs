use super::*;

fn binding() -> Binding {
    Binding {
        instance_id: "fixture".into(),
        profile: "alpha".into(),
        native_id: "native".into(),
        launch_id: "launch".into(),
    }
}
fn identity() -> Identity {
    Identity {
        native_id: "native".into(),
        file: "a".repeat(64),
        parent: None,
    }
}
fn row(id: &str) -> Entry {
    Entry {
        id: id.into(),
        parent_id: None,
        kind: Kind::Assistant,
        model: Some(Model {
            provider: "fixture".into(),
            model: "model".into(),
        }),
        counters: Some(Counters {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write_1h: 1,
            reasoning: 1,
        }),
        gap: None,
        sdk_cost_nano: None,
    }
}
fn source(rows: Vec<Entry>) -> Source {
    let cursor = rows
        .iter()
        .try_fold(Cursor::default(), |c, e| c.append(e))
        .unwrap();
    let high_water = cursor.count;
    Source {
        binding: binding(),
        generation: uuid::Uuid::new_v4().to_string(),
        identity: identity(),
        entries: rows,
        cursor,
        high_water,
        announced: BTreeMap::new(),
        gap: None,
    }
}
fn policy() -> Policy {
    Policy {
        version: 1,
        basis: "observed-native-entries".into(),
        tokens: Some(100),
        usd: Some(Valuation {
            currency: "USD".into(),
            basis: "operator-fixed-rates".into(),
            limit_nano: 100,
            quotes: vec![Quote {
                model: row("a").model.unwrap(),
                input: 1,
                output: 1,
                cache_read: 1,
                cache_write: 1,
                cache_write_1h: Some(1),
            }],
        }),
    }
}
#[test]
fn report_replay_prefixes_changed_ids_and_generation_do_not_refund() {
    let all = source(vec![row("a"), row("b"), row("c")]);
    let mut s = Source::new(binding(), all.generation.clone(), identity());
    let first = Report {
        version: 1,
        challenge: uuid::Uuid::new_v4().to_string(),
        identity: identity(),
        after: Cursor::default(),
        entries: all.entries[..2].to_vec(),
        head: all.cursor.clone(),
    };
    assert!(s.reconcile(&binding(), &all.generation, &first).unwrap());
    assert_eq!(s.cursor.count, 2);
    assert!(!s.reconcile(&binding(), &all.generation, &first).unwrap());
    let last = Report {
        after: s.cursor.clone(),
        entries: all.entries[2..].to_vec(),
        ..first.clone()
    };
    assert!(s.reconcile(&binding(), &all.generation, &last).unwrap());
    assert_eq!(s, all);
    assert!(!s.reconcile(&binding(), &all.generation, &first).unwrap());
    let unchanged = s.clone();
    for change in ["prefix", "id", "value", "head", "identity"] {
        let mut bad = first.clone();
        match change {
            "prefix" => bad.after.hash = "0".repeat(64),
            "id" => bad.entries[1].id = "a".into(),
            "value" => bad.entries[0].counters.as_mut().unwrap().input = 7,
            "head" => {
                bad.head.count = 1;
                bad.entries.truncate(1);
                bad.head.hash = "0".repeat(64);
            }
            _ => bad.identity.file = "b".repeat(64),
        }
        assert!(
            s.reconcile(&binding(), &all.generation, &bad).is_err(),
            "{change}"
        );
        assert_eq!(s, unchanged);
    }
    let next = uuid::Uuid::new_v4().to_string();
    let heartbeat = Report {
        after: s.cursor.clone(),
        entries: vec![],
        ..first
    };
    assert!(s.reconcile(&binding(), &next, &heartbeat).unwrap());
    assert_eq!(s.entries, all.entries);
    assert_eq!(s.generation, next);
    let mut duplicate = all.clone();
    duplicate.entries.push(row("a"));
    duplicate.cursor = duplicate.cursor.append(&row("a")).unwrap();
    duplicate.high_water = duplicate.cursor.count;
    assert!(duplicate.validate().is_err());
}
#[test]
fn announced_incomplete_tail_cannot_disappear_or_change_on_later_pages() {
    let all = source(vec![row("a"), row("b"), row("c")]);
    let mut s = Source::new(binding(), all.generation.clone(), identity());
    let first = Report {
        version: 1,
        challenge: uuid::Uuid::new_v4().to_string(),
        identity: identity(),
        after: Cursor::default(),
        entries: all.entries[..2].to_vec(),
        head: all.cursor.clone(),
    };
    s.reconcile(&binding(), &all.generation, &first).unwrap();
    let truncated = Report {
        after: s.cursor.clone(),
        entries: vec![],
        head: s.cursor.clone(),
        ..first.clone()
    };
    assert!(
        s.reconcile(&binding(), &all.generation, &truncated)
            .is_err(),
        "announced source suffix disappeared"
    );
    let mut changed = all.clone();
    changed.entries[2].counters.as_mut().unwrap().input = 10;
    changed.entries.push(row("d"));
    changed.cursor = changed
        .entries
        .iter()
        .try_fold(Cursor::default(), |c, e| c.append(e))
        .unwrap();
    let changed = Report {
        after: s.cursor.clone(),
        entries: changed.entries[2..].to_vec(),
        head: changed.cursor,
        ..first.clone()
    };
    assert!(
        s.reconcile(&binding(), &all.generation, &changed).is_err(),
        "larger head erased an earlier unreconciled prefix promise"
    );
    let valid = Report {
        after: s.cursor.clone(),
        entries: all.entries[2..].to_vec(),
        ..first
    };
    s.reconcile(&binding(), &all.generation, &valid).unwrap();
    assert_eq!(s.cursor, all.cursor);
}

#[test]
fn component_tokens_and_per_entry_valuation_are_independent_of_chunking() {
    let s = source(vec![row("a"), row("b")]);
    let mut p = policy();
    p.tokens = Some(20);
    p.usd.as_mut().unwrap().limit_nano = 3;
    let whole = measure(&s, &Cursor::default(), &s.cursor, Some(&p)).unwrap();
    assert_eq!(whole.reported_tokens, 20);
    assert_eq!(whole.operator_estimate_nano, Some(2));
    assert_eq!(whole.deny(&p), Some(Denial::Exhausted));
    let middle = s.prefix(1).unwrap();
    let a = measure(&s, &Cursor::default(), &middle, Some(&p)).unwrap();
    let b = measure(&s, &middle, &s.cursor, Some(&p)).unwrap();
    assert_eq!(
        whole.operator_estimate_nano.unwrap(),
        a.operator_estimate_nano.unwrap() + b.operator_estimate_nano.unwrap()
    );
    p.tokens = None;
    p.usd.as_mut().unwrap().limit_nano = 2;
    assert_eq!(
        whole.deny(&p),
        Some(Denial::Exhausted),
        "nonzero operator valuation threshold is enforced"
    );
    let mut priced = p.clone();
    let q = &mut priced.usd.as_mut().unwrap().quotes[0];
    q.input = 1_000_000;
    q.output = 2_000_000;
    q.cache_read = 3_000_000;
    q.cache_write = 4_000_000;
    q.cache_write_1h = Some(8_000_000);
    assert_eq!(
        measure(&s, &Cursor::default(), &middle, Some(&priced))
            .unwrap()
            .operator_estimate_nano,
        Some(1 + 4 + 9 + 12 + 8)
    );
    let empty = measure(&s, &s.cursor, &s.cursor, Some(&priced)).unwrap();
    assert_eq!(empty.reported_tokens, 0);
    assert!(empty.tokens_complete);
}
#[test]
fn zero_quotes_missing_summary_models_rollups_and_aborts_are_not_free_measurements() {
    for case in [
        "zero_quote",
        "missing_quote",
        "summary",
        "missing_usage",
        "nested",
        "abort",
        "reported_zero",
    ] {
        let mut r = row("a");
        let mut p = policy();
        let q = &mut p.usd.as_mut().unwrap().quotes[0];
        q.input = 0;
        q.output = 0;
        q.cache_read = 0;
        q.cache_write = 0;
        q.cache_write_1h = Some(0);
        match case {
            "missing_quote" => r.model.as_mut().unwrap().model = "unknown".into(),
            "summary" => {
                r.kind = Kind::Compaction;
                r.model = None;
            }
            "missing_usage" => {
                r.counters = None;
                r.gap = Some(Gap::MissingUsage);
            }
            "nested" => {
                r.kind = Kind::Nested;
                r.counters = None;
                r.gap = Some(Gap::NestedOverlap);
            }
            "abort" => {
                r.counters = Some(Counters::default());
                r.gap = Some(Gap::ErrorOrAbort);
            }
            "reported_zero" => r.counters = Some(Counters::default()),
            _ => {}
        }
        let s = source(vec![r]);
        s.validate().unwrap();
        let m = measure(&s, &Cursor::default(), &s.cursor, Some(&p)).unwrap();
        assert_eq!(
            m.deny(&p).is_none(),
            matches!(case, "zero_quote" | "reported_zero"),
            "{case}"
        );
        if matches!(case, "missing_quote" | "summary") {
            assert!(m.tokens_complete);
            assert!(!m.valuation_complete);
            p.usd = None;
            assert_eq!(m.deny(&p), None);
        }
        assert_eq!(
            m.sdk_recorded_nano, None,
            "SDK price omission isn't actual zero cost"
        );
    }
}
#[test]
fn invalid_currency_overflow_and_token_subsets_fail_without_saturation() {
    let s = source(vec![row("a")]);
    for case in ["currency", "mode", "duplicate", "rate", "limit"] {
        let mut p = policy();
        let usd = p.usd.as_mut().unwrap();
        match case {
            "currency" => usd.currency = "EUR".into(),
            "mode" => usd.basis = "invoice".into(),
            "duplicate" => usd.quotes.push(usd.quotes[0].clone()),
            "rate" => usd.quotes[0].input = SAFE + 1,
            _ => p.tokens = Some(0),
        }
        assert!(p.validate().is_err(), "{case}");
        assert!(measure(&s, &Cursor::default(), &s.cursor, Some(&p)).is_err());
    }
    for c in [
        Counters {
            input: SAFE,
            output: 1,
            ..Counters::default()
        },
        Counters {
            reasoning: 1,
            ..Counters::default()
        },
        Counters {
            cache_write_1h: 1,
            ..Counters::default()
        },
    ] {
        assert!(c.total().is_err());
    }
    let mut q = policy().usd.unwrap().quotes.remove(0);
    q.input = SAFE;
    assert!(q
        .value(&Counters {
            input: SAFE,
            ..Counters::default()
        })
        .is_err());
}
