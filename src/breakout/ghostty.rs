//! Ghostty driver for Breakout: AppleScript generation and the `osascript` runner.
#![cfg(target_os = "macos")]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::grid::Rect;
use super::request::Surface;

/// Escape text for an AppleScript string expression. Quotes and backslashes are
/// escaped; line breaks are encoded as `linefeed` / `return` so none goes through raw.
fn as_literal(text: &str) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    if !text.contains(['\n', '\r']) {
        return quote(text);
    }
    let mut parts = Vec::new();
    let mut segment = String::new();
    for c in text.chars() {
        match c {
            '\n' | '\r' => {
                parts.push(quote(&segment));
                parts.push(if c == '\n' { "linefeed" } else { "return" }.to_string());
                segment.clear();
            }
            _ => segment.push(c),
        }
    }
    parts.push(quote(&segment));
    format!("({})", parts.join(" & "))
}

/// Every script bounds its Apple events so a missing consent cannot hang for minutes.
fn with_timeout(body: &str) -> String {
    format!("with timeout of 10 seconds\n{body}end timeout\n")
}

/// Open one Ghostty window per surface, each running its command under a title.
pub fn open_windows_script(surfaces: &[Surface]) -> String {
    let mut script = String::from("tell application \"Ghostty\"\n");
    for surface in surfaces {
        script.push_str("    set cfg to new surface configuration\n");
        script.push_str(&format!(
            "    set command of cfg to {}\n",
            as_literal(&surface.command)
        ));
        script.push_str("    set win to new window with configuration cfg\n");
        script.push_str(&format!(
            "    set name override of (focused terminal of selected tab of win) to {}\n",
            as_literal(&surface.title)
        ));
    }
    script.push_str("end tell\n");
    with_timeout(&script)
}

/// Move, size and raise windows through Accessibility, found by their titles.
/// Titles use the first 8 id characters, so two Sessions sharing a title and id
/// prefix would resolve to the same window.
pub fn place_script(placements: &[(String, Rect)]) -> String {
    let mut script =
        String::from("tell application \"System Events\"\n    tell process \"Ghostty\"\n");
    for (title, rect) in placements {
        script.push_str(&format!(
            "        set w to first window whose name is {}\n",
            as_literal(title)
        ));
        script.push_str(&format!(
            "        set position of w to {{{}, {}}}\n",
            rect.x, rect.y
        ));
        script.push_str(&format!(
            "        set size of w to {{{}, {}}}\n",
            rect.width, rect.height
        ));
        script.push_str("        perform action \"AXRaise\" of w\n");
    }
    script.push_str("    end tell\nend tell\n");
    with_timeout(&script)
}

/// Close the windows a Breakout opened. Ids are collected first and each window is
/// closed by id, because a live `every window whose ...` filter shrinks as windows
/// close. A missing id (error -1719) means the window is already gone.
pub fn close_script(titles: &[String]) -> String {
    let mut script = String::from("tell application \"Ghostty\"\n");
    for title in titles {
        script.push_str(&format!(
            "    set winIds to id of every window whose name is {}\n",
            as_literal(title)
        ));
        script.push_str(concat!(
            "    repeat with i in winIds\n",
            "        try\n",
            "            close window (first window whose id is (contents of i))\n",
            "        on error errMsg number errNum\n",
            "            if errNum is not -1719 then error errMsg number errNum\n",
            "        end try\n",
            "    end repeat\n",
        ));
    }
    script.push_str("end tell\n");
    with_timeout(&script)
}

/// JXA: the usable frame of the screen with keyboard focus, as top-left-origin JSON.
pub const FRAME_SCRIPT: &str = r#"ObjC.import('AppKit');
const primaryHeight = $.NSScreen.screens.objectAtIndex(0).frame.size.height;
const v = $.NSScreen.mainScreen.visibleFrame;
JSON.stringify({
  x: Math.round(v.origin.x),
  y: Math.round(primaryHeight - (v.origin.y + v.size.height)),
  width: Math.round(v.size.width),
  height: Math.round(v.size.height),
});
"#;

/// Turn `osascript` stderr into the message shown to the user. A timeout (-1712)
/// or refusal (-1743) almost always means Automation consent is missing.
fn friendly_error(stderr: &str) -> String {
    if stderr.contains("-1712") || stderr.contains("-1743") {
        format!(
            "macOS did not let aoe control Ghostty. Allow aoe (or your terminal) under \
             System Settings > Privacy & Security > Automation, then try again. ({stderr})"
        )
    } else {
        stderr.to_string()
    }
}

const OSASCRIPT_WAIT: Duration = Duration::from_secs(15);

/// Run a script on `osascript`'s stdin. Returns stdout, or the error text.
fn osascript(language_args: &[&str], script: &str) -> Result<String, String> {
    let mut child = Command::new("osascript")
        .args(language_args)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("osascript spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(script.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("osascript stdin: {e}"));
        }
    }
    let deadline = Instant::now() + OSASCRIPT_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("osascript timed out after {OSASCRIPT_WAIT:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("osascript wait: {e}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("osascript output: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(friendly_error(
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    }
}

/// Whether Ghostty is installed where macOS can launch it.
pub fn ghostty_present() -> bool {
    Command::new("open")
        .args(["-Ra", "Ghostty"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Open the surfaces as separate windows and tile them. Any error while placing
/// leaves the windows open untiled and is carried in the outcome's note.
pub fn run_windows(surfaces: &[Surface]) -> Result<super::BreakoutOutcome, String> {
    osascript(&[], &open_windows_script(surfaces))?;
    let titles: Vec<String> = surfaces.iter().map(|s| s.title.clone()).collect();
    let mut outcome = super::BreakoutOutcome {
        opened: surfaces.len(),
        tiled: false,
        titles: titles.clone(),
        note: None,
    };
    let placed = osascript(&["-l", "JavaScript"], FRAME_SCRIPT)
        .and_then(|json| {
            serde_json::from_str::<Rect>(&json).map_err(|e| format!("screen frame: {e}"))
        })
        .and_then(|screen| {
            super::grid::tile(surfaces.len(), screen).ok_or_else(|| "no grid".to_string())
        })
        .and_then(|cells| {
            let placements: Vec<(String, Rect)> = titles.into_iter().zip(cells).collect();
            osascript(&[], &place_script(&placements)).map(|_| ())
        });
    match placed {
        Ok(()) => outcome.tiled = true,
        Err(error) => outcome.note = Some(format!("The windows are open but not tiled: {error}")),
    }
    Ok(outcome)
}

pub fn close(titles: &[String]) -> Result<(), String> {
    osascript(&[], &close_script(titles)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(title: &str, command: &str) -> Surface {
        Surface {
            title: title.to_string(),
            command: command.to_string(),
        }
    }

    #[test]
    fn literals_escape_quotes_and_backslashes() {
        assert_eq!(as_literal(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }

    #[test]
    fn literals_never_carry_raw_line_breaks() {
        let lit = as_literal("a\nb\rc\"d");
        assert!(!lit.contains('\n') && !lit.contains('\r'), "{lit:?}");
        assert_eq!(lit, "(\"a\" & linefeed & \"b\" & return & \"c\\\"d\")");
    }

    #[test]
    fn scripts_are_wrapped_in_a_timeout() {
        for script in [
            open_windows_script(&[surface("t", "c")]),
            place_script(&[(
                "t".into(),
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            )]),
            close_script(&["t".to_string()]),
        ] {
            assert!(
                script.starts_with("with timeout of 10 seconds\n"),
                "{script}"
            );
            assert!(script.trim_end().ends_with("end timeout"), "{script}");
        }
    }

    #[test]
    fn open_script_creates_one_titled_window_per_surface() {
        let script = open_windows_script(&[
            surface(
                "aoe breakout \u{b7} A \u{b7} 1",
                "'/x/aoe' session attach 1",
            ),
            surface(
                "aoe breakout \u{b7} B \u{b7} 2",
                "ssh -t bender aoe session attach 2",
            ),
        ]);
        assert_eq!(script.matches("new window with configuration").count(), 2);
        assert!(script.contains(r#"set command of cfg to "'/x/aoe' session attach 1""#));
        assert!(script.contains("set name override of (focused terminal of selected tab of win) to \"aoe breakout \u{b7} B \u{b7} 2\""));
        assert!(script.contains("tell application \"Ghostty\""));
    }

    #[test]
    fn open_script_escapes_every_interpolated_string() {
        let script = open_windows_script(&[surface("ti\"tle\n", "'/a \"b\"/aoe' x\\y")]);
        assert!(
            script.contains(r#"set command of cfg to "'/a \"b\"/aoe' x\\y""#),
            "{script}"
        );
        assert!(
            script.contains("(\"ti\\\"tle\" & linefeed & \"\")"),
            "{script}"
        );
    }

    #[test]
    fn place_script_sets_position_size_and_raises_by_title() {
        let script = place_script(&[(
            "aoe breakout \u{b7} A \u{b7} 1".to_string(),
            Rect {
                x: -960,
                y: 38,
                width: 864,
                height: 1079,
            },
        )]);
        assert!(script.contains("tell process \"Ghostty\""));
        assert!(script.contains("first window whose name is \"aoe breakout \u{b7} A \u{b7} 1\""));
        assert!(script.contains("set position of w to {-960, 38}"));
        assert!(script.contains("set size of w to {864, 1079}"));
        assert!(script.contains("perform action \"AXRaise\" of w"));
    }

    #[test]
    fn close_script_closes_by_id_never_by_live_filter() {
        let script = close_script(&["t1".to_string(), "t\"2".to_string()]);
        assert!(script.contains("id of every window whose name is \"t1\""));
        assert!(script.contains("id of every window whose name is \"t\\\"2\""));
        assert!(script.contains("close window (first window whose id is (contents of i))"));
        assert!(!script.contains("close window w"));
        assert!(
            script.contains("-1719"),
            "a missing id must count as already closed"
        );
    }

    #[test]
    fn timeout_and_authorisation_errors_point_at_automation_settings() {
        for text in [
            "execution error: Ghostty got an error: AppleEvent timed out. (-1712)",
            "execution error: Not authorized to send Apple events to Ghostty. (-1743)",
        ] {
            let message = friendly_error(text);
            assert!(
                message.contains("Privacy & Security > Automation"),
                "{message}"
            );
        }
        let other = "execution error: Can't get window 1. (-1719)";
        assert_eq!(friendly_error(other), other);
    }
}
