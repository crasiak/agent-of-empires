use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use std::io::{self, Write};

pub(super) fn enable(output: &mut impl Write) -> io::Result<()> {
    crossterm::execute!(
        output,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
    )
}

pub(super) fn disable(output: &mut impl Write) -> io::Result<()> {
    crossterm::execute!(output, PopKeyboardEnhancementFlags)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
    use std::path::Path;
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const TEST: &str = "tui::keyboard::tests::tmux_fallback_submit_survives_without_extended_keys";
    const PROBE: &str = "AOE_KEYBOARD_TEST_PROBE";

    struct Tmux {
        socket: std::path::PathBuf,
    }

    impl Tmux {
        fn run(&self, args: &[&str]) -> Output {
            crate::process::run_with_timeout(
                Command::new("tmux")
                    .arg("-S")
                    .arg(&self.socket)
                    .args(args)
                    .stdin(Stdio::null()),
                Duration::from_secs(5),
            )
            .expect("run private tmux")
            .expect("private tmux command deadline")
        }

        fn check(&self, args: &[&str]) {
            let output = self.run(args);
            assert!(output.status.success(), "{args:?}: {output:?}");
        }

        fn wait(&self, path: &Path, exit: &Path) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !path.exists() {
                assert!(
                    !exit.exists() && Instant::now() < deadline,
                    "waiting for {path:?}: {:?}",
                    self.run(&["capture-pane", "-p", "-t", "keys:probe"])
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Tmux {
        fn drop(&mut self) {
            let _ = self.run(&["kill-server"]);
        }
    }

    fn probe(root: &Path) {
        crossterm::terminal::enable_raw_mode().unwrap();
        let mut output = io::stdout();
        for (phase, enabled) in [("enabled", true), ("disabled", false), ("resumed", true)] {
            if enabled {
                enable(&mut output).unwrap();
            } else {
                disable(&mut output).unwrap();
            }
            // The cursor reply fences tmux's processing of the preceding mode change.
            crossterm::cursor::position().unwrap();
            for (key, code) in [("enter", KeyCode::Enter), ("submit", KeyCode::F(4))] {
                std::fs::write(root.join(format!("{phase}-{key}")), "ready").unwrap();
                assert!(event::poll(Duration::from_secs(5)).unwrap());
                assert_eq!(
                    event::read().unwrap(),
                    Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
                    "{phase} {key}"
                );
            }
        }
        disable(&mut output).unwrap();
        crossterm::terminal::disable_raw_mode().unwrap();
    }

    #[test]
    fn tmux_fallback_submit_survives_without_extended_keys() {
        if std::env::var_os(PROBE).is_some() {
            probe(Path::new(&std::env::var("AOE_KEYBOARD_TEST_ROOT").unwrap()));
            return;
        }
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("tmux unavailable; skipping key transport probe");
            return;
        }
        let root = tempfile::Builder::new()
            .prefix("aoe-keys-")
            .tempdir_in("/tmp")
            .unwrap();
        let tmux = Tmux {
            socket: root.path().join("s"),
        };
        tmux.check(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "keys",
            "sleep 120",
        ]);
        tmux.check(&["set-option", "-g", "default-shell", "/bin/sh"]);
        tmux.check(&["set-window-option", "-g", "remain-on-exit", "on"]);
        for mode in ["on", "off"] {
            tmux.check(&["set-option", "-s", "extended-keys", mode]);
            let case = root.path().join(mode);
            std::fs::create_dir(&case).unwrap();
            let exit = case.join("exit");
            let quote = |path: &Path| shell_words::quote(path.to_str().unwrap()).into_owned();
            let command = format!(
                "env {PROBE}={mode} AOE_KEYBOARD_TEST_ROOT={} {} --exact {TEST} --nocapture; printf '%s' \"$?\" > {}",
                quote(&case), quote(&std::env::current_exe().unwrap()), quote(&exit)
            );
            tmux.check(&["new-window", "-d", "-n", "probe", &command]);
            for phase in ["enabled", "disabled", "resumed"] {
                for (key, input) in [("enter", "C-Enter"), ("submit", "F4")] {
                    tmux.wait(&case.join(format!("{phase}-{key}")), &exit);
                    tmux.check(&["send-keys", "-t", "keys:probe", input]);
                }
            }
            tmux.wait(&exit, &exit);
            assert_eq!(std::fs::read_to_string(exit).unwrap(), "0", "{mode}");
            tmux.check(&["kill-window", "-t", "keys:probe"]);
        }
    }
}
