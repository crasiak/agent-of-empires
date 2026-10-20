use super::*;

pub(in crate::session::afk::authority) const POLICY: &str = r#"{"version":1,"binding":{"instance_id":"fixture","profile":"alpha","native_id":"native","launch_id":"launch"},"revision":7,"window":"00000000-0000-4000-8000-000000000001","generation":"00000000-0000-4000-8000-000000000002","enabled":true,"expires_at_ms":61000,"issued_at_ms":1000,"duration_ms":60000,"mode":"control-only","allowance":0}"#;
pub(in crate::session::afk::authority) const BOOK: &str = r#"{"version":4,"windows":[{"version":4,"binding":{"instance_id":"fixture","profile":"alpha","native_id":"native","launch_id":"launch"},"generation":"00000000-0000-4000-8000-000000000002","id":"00000000-0000-4000-8000-000000000003","revision":1,"root":"/workspace","control_root":"/app","root_identity":[1,2],"grant":{"version":4,"task":"scratch","scope":"create scratch","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"question_deferrals":1,"assurance":"stock-sdk-main-loop-reservations"},"issued_at_ms":1000,"expires_at_ms":61000,"state":"pending","confirmed":true,"reservations":[],"reads":0,"records":[],"permit":null,"read_facts":[],"checkpoints":[],"nudges":[],"terminal_reason":null,"question":{"identity":{"operation":"00000000-0000-4000-8000-000000000004","interaction_id":"00000000-0000-4000-8000-000000000001","tool_call_id":"call","context_ref":"context","kind":"input","arguments_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"admitted_at_ms":2000,"emission_intent_at_ms":2001,"outcome":"unknown","outcome_at_ms":2002,"observed_request":null,"recovered_at_ms":null}}]}"#;
pub(in crate::session::afk::authority) const SNAPSHOT: &str = r#"{"version":1,"profile":"alpha","revision":1,"sessions":{"fixture":{"epoch":1,"policy":{"version":1,"binding":{"instance_id":"fixture","profile":"alpha","native_id":"native","launch_id":"launch"},"revision":8,"window":"00000000-0000-4000-8000-000000000001","generation":"00000000-0000-4000-8000-000000000002","enabled":false,"expires_at_ms":null,"issued_at_ms":1000,"duration_ms":0,"mode":"control-only","allowance":0},"book":{"version":4,"windows":[{"version":4,"binding":{"instance_id":"fixture","profile":"alpha","native_id":"native","launch_id":"launch"},"generation":"00000000-0000-4000-8000-000000000002","id":"00000000-0000-4000-8000-000000000003","revision":1,"root":"/workspace","control_root":"/app","root_identity":[1,2],"grant":{"version":4,"task":"scratch","scope":"create scratch","files":[{"path":"scratch.txt","capability":"create"}],"requests":2,"settlement_nudges":0,"question_deferrals":1,"assurance":"stock-sdk-main-loop-reservations"},"issued_at_ms":1000,"expires_at_ms":61000,"state":"ended","confirmed":false,"reservations":[],"reads":0,"records":[],"permit":null,"read_facts":[],"checkpoints":[],"nudges":[],"terminal_reason":"delivery_unknown","question":{"identity":{"operation":"00000000-0000-4000-8000-000000000004","tool_call_id":"call","interaction_id":"00000000-0000-4000-8000-000000000001","kind":"input","arguments_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","context_ref":"context"},"admitted_at_ms":2000,"emission_intent_at_ms":2001,"outcome":"unknown","outcome_at_ms":2002,"observed_request":null,"recovered_at_ms":null}}]},"evidence":[{"namespace":"afk","instance":"fixture","version":1,"sha256":"3c547ca86178dc513e7da517172d3088b987aa9bbd42e1ebdd01cb0ca9ea4ec4","original":"afk-authority-v1/migration/original-0000.json","profiles":["alpha"]},{"namespace":"afk-runtime-v4","instance":"fixture","version":4,"sha256":"4de0ad951a74ba3cf8ef604e6260ea6a671e262afa9520f14f473da362a9a06c","original":"afk-authority-v1/migration/original-0001.json","profiles":["alpha"]}]}}}"#;

#[test]
fn literal_prior_binary_import_survives_interruption_without_hash_drift() {
    for phase in [
        Phase::Inventory,
        Phase::Originals,
        Phase::Fences,
        Phase::Imports,
        Phase::Complete,
    ] {
        let app = tests::isolated();
        for (namespace, leaf, bytes) in [
            ("afk", "policy.json", POLICY),
            ("afk-runtime-v4", "ledger.json", BOOK),
        ] {
            let source = Store::open_namespace(
                app.path(),
                "fixture",
                true,
                namespace,
                delegation::SESSION_BYTES,
            )
            .unwrap();
            source.write_bytes(leaf, bytes.as_bytes()).unwrap();
        }
        INTERRUPT.with(|at| at.set(Some(phase)));
        let interrupted = run_at(app.path());
        INTERRUPT.with(|at| at.set(None));
        assert!(interrupted.is_err(), "{phase:?}");
        run_at(app.path()).unwrap();
        run_at(app.path()).unwrap();
        let bytes = profile_store(app.path(), "alpha", false)
            .unwrap()
            .bytes("snapshot.json")
            .unwrap()
            .unwrap();
        assert_eq!(
            bytes,
            SNAPSHOT.as_bytes(),
            "literal prior-binary bytes differ after {phase:?}"
        );
        assert_eq!(
            manifest(app.path()).unwrap().imports["alpha"],
            "f6ba50ee5c3f424450f90b4b1a60126d020984db2c8520c4062920ff5492bc08"
        );
        let originals = migration_store(app.path(), false).unwrap();
        assert_eq!(
            originals.bytes("original-0000.json").unwrap().unwrap(),
            POLICY.as_bytes()
        );
        assert_eq!(
            originals.bytes("original-0001.json").unwrap().unwrap(),
            BOOK.as_bytes()
        );
    }
}
