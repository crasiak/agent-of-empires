use serial_test::parallel;
use std::process::Command;

#[test]
#[parallel]
fn observation_clock_bypasses_startup_and_refuses_arguments_without_state_writes() {
    let root = tempfile::tempdir().unwrap();
    let mut previous = None;
    for extra in [None, None, Some("unexpected")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aoe"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", root.path())
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("AOE_LOG_LEVEL", "debug")
            .env("AOE_DAEMON_URL", "http://invalid.invalid")
            .current_dir(root.path())
            .arg("__afk-observation-clock");
        if let Some(extra) = extra {
            command.arg(extra);
        }
        let output = command.output().unwrap();
        if extra.is_some() {
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        } else {
            assert!(output.status.success(), "{:?}", output.stderr);
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(value["boot"].as_str().is_some_and(|s| !s.is_empty()));
            let millis = value["millis"].as_u64().unwrap();
            if let Some((boot, prior)) = &previous {
                assert_eq!(&value["boot"], boot);
                assert!(millis >= *prior);
            }
            previous = Some((value["boot"].clone(), millis));
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
