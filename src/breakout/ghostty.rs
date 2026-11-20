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
/// The script keeps the ids of the windows it creates; if anything fails it closes
/// those ids (never by title, which could hit a window from an earlier Breakout)
/// and re-raises the original error with its number.
pub fn open_windows_script(surfaces: &[Surface]) -> String {
    let mut script = String::from(concat!(
        "tell application \"Ghostty\"\n",
        "    set createdIds to {}\n",
        "    try\n",
    ));
    for surface in surfaces {
        script.push_str("        set cfg to new surface configuration\n");
        script.push_str(&format!(
            "        set command of cfg to {}\n",
            as_literal(&surface.command)
        ));
        script.push_str("        set win to new window with configuration cfg\n");
        script.push_str("        set end of createdIds to id of win\n");
        script.push_str(&format!(
            "        set name override of (focused terminal of selected tab of win) to {}\n",
            as_literal(&surface.title)
        ));
    }
    script.push_str(concat!(
        "    on error errMsg number errNum\n",
        "        repeat with i in createdIds\n",
        "            try\n",
        "                close window (first window whose id is (contents of i))\n",
        "            end try\n",
        "        end repeat\n",
        "        error errMsg number errNum\n",
        "    end try\n",
        "end tell\n",
    ));
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
/// close. A failed lookup of one id means the window is already gone, so every
/// error is swallowed except -1712 and -1743 (no Automation consent).
/// Breaking out the same Session twice while the first windows are open gives
/// both windows one title, so close matches both and placement hits the older one.
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
            "            if errNum is -1712 or errNum is -1743 then error errMsg number errNum\n",
            "        end try\n",
            "    end repeat\n",
        ));
    }
    script.push_str("end tell\n");
    with_timeout(&script)
}

/// Close every Breakout window, found by the title prefix, except those whose
/// title ends with one of the kept id prefixes. Ids and names are collected
/// before anything closes, then each window is closed by id with the same error
/// rules as `close_script`. The script returns how many windows it closed.
pub fn sweep_script(keep_id_prefixes: &[String]) -> String {
    let keep: Vec<String> = keep_id_prefixes
        .iter()
        .map(|prefix| as_literal(&super::title_suffix(prefix)))
        .collect();
    let mut script = String::from("tell application \"Ghostty\"\n");
    script.push_str(&format!(
        "    set keepSuffixes to {{{}}}\n",
        keep.join(", ")
    ));
    script.push_str(&format!(
        "    set winIds to id of every window whose name starts with {}\n",
        as_literal(super::TITLE_PREFIX)
    ));
    script.push_str(concat!(
        "    set closeIds to {}\n",
        "    repeat with i in winIds\n",
        "        try\n",
        "            set winName to name of (first window whose id is (contents of i))\n",
        "            set keep to false\n",
        "            considering case\n",
        "                repeat with s in keepSuffixes\n",
        "                    if winName ends with (contents of s) then set keep to true\n",
        "                end repeat\n",
        "            end considering\n",
        "            if not keep then set end of closeIds to (contents of i)\n",
        "        on error errMsg number errNum\n",
        "            if errNum is -1712 or errNum is -1743 then error errMsg number errNum\n",
        "        end try\n",
        "    end repeat\n",
        "    set closedCount to 0\n",
        "    repeat with i in closeIds\n",
        "        try\n",
        "            close window (first window whose id is (contents of i))\n",
        "            set closedCount to closedCount + 1\n",
        "        on error errMsg number errNum\n",
        "            if errNum is -1712 or errNum is -1743 then error errMsg number errNum\n",
        "        end try\n",
        "    end repeat\n",
        "    return closedCount\n",
        "end tell\n",
    ));
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

/// Start of the error text when the runner killed `osascript` on its overall wait.
const KILLED_PREFIX: &str = "osascript timed out after";

/// What to tell the user about windows after a failed open. The script closes
/// the ids it created on its own errors; a killed `osascript` cannot, and no
/// title-based cleanup is attempted either way.
fn open_failure_note(error: &str) -> String {
    if error.starts_with(KILLED_PREFIX) {
        "some windows may still be open; use Close breakout windows to remove them".to_string()
    } else {
        "windows it had opened were closed where possible".to_string()
    }
}

/// Overall wait for a script: a base plus a share per window it touches, so many
/// slow `new window` events do not get killed while still succeeding.
fn wait_for(windows: usize) -> Duration {
    Duration::from_secs(15 + 5 * windows as u64)
}

/// Run a script on `osascript`'s stdin. Returns stdout, or the error text.
/// The pipes are not drained while polling; our scripts print almost nothing.
fn osascript(language_args: &[&str], script: &str, wait: Duration) -> Result<String, String> {
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
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{KILLED_PREFIX} {wait:?}"));
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

/// The outcome once placement has been tried. Any placement error leaves the
/// windows open and carries its text; some may already have been placed.
fn outcome_after_placement(
    titles: Vec<String>,
    placed: Result<(), String>,
) -> super::BreakoutOutcome {
    let (tiled, note) = match placed {
        Ok(()) => (true, None),
        Err(error) => (
            false,
            Some(format!(
                "The windows are open but not tiled (some may be partly placed): {error}"
            )),
        ),
    };
    super::BreakoutOutcome {
        opened: titles.len(),
        tiled,
        titles,
        note,
    }
}

/// Open the surfaces as separate windows and tile them. Any error while placing
/// leaves the windows open untiled and is carried in the outcome's note. If the
/// open script itself fails, the script closes the ids it created.
pub fn run_windows(surfaces: &[Surface]) -> Result<super::BreakoutOutcome, String> {
    let titles: Vec<String> = surfaces.iter().map(|s| s.title.clone()).collect();
    if let Err(error) = osascript(
        &[],
        &open_windows_script(surfaces),
        wait_for(surfaces.len()),
    ) {
        return Err(format!("{error} ({})", open_failure_note(&error)));
    }
    let placed = osascript(&["-l", "JavaScript"], FRAME_SCRIPT, wait_for(0))
        .and_then(|json| {
            serde_json::from_str::<Rect>(&json).map_err(|e| format!("screen frame: {e}"))
        })
        .and_then(|screen| {
            super::grid::tile(surfaces.len(), screen).ok_or_else(|| "no grid".to_string())
        })
        .and_then(|cells| {
            let placements: Vec<(String, Rect)> = titles.iter().cloned().zip(cells).collect();
            osascript(&[], &place_script(&placements), wait_for(placements.len())).map(|_| ())
        });
    Ok(outcome_after_placement(titles, placed))
}

pub fn close(titles: &[String]) -> Result<(), String> {
    osascript(&[], &close_script(titles), wait_for(titles.len())).map(|_| ())
}

/// Run the sweep. Its window count is unknown up front, so it waits as long as
/// the largest Breakout would.
pub fn sweep(keep_id_prefixes: &[String]) -> Result<usize, String> {
    let output = osascript(
        &[],
        &sweep_script(keep_id_prefixes),
        wait_for(super::request::MAX_SESSIONS),
    )?;
    output
        .parse()
        .map_err(|_| format!("unexpected sweep output: {output}"))
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
            sweep_script(&[]),
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
            script.contains("if errNum is -1712 or errNum is -1743 then error errMsg"),
            "only consent errors may abort; any lookup failure means already closed"
        );
    }

    #[test]
    fn sweep_script_finds_by_prefix_and_closes_by_id() {
        let script = sweep_script(&[]);
        assert!(script.contains(
            "set winIds to id of every window whose name starts with \"aoe breakout \u{b7} \""
        ));
        assert!(script.contains("set keepSuffixes to {}"));
        assert!(script.contains("close window (first window whose id is (contents of i))"));
        assert!(!script.contains("close window w"));
        let (collect, close) = script.split_once("set closedCount to 0").unwrap();
        assert!(
            !collect.contains("close window"),
            "nothing closes while collecting"
        );
        assert!(close.contains("repeat with i in closeIds"));
        assert!(script.contains("if errNum is -1712 or errNum is -1743 then error errMsg"));
        assert!(script.contains("return closedCount"));
    }

    #[test]
    fn sweep_script_skips_kept_id_prefixes() {
        let script = sweep_script(&["a1b2c3d4".to_string(), "x\"y".to_string()]);
        assert!(
            script.contains("set keepSuffixes to {\" \u{b7} a1b2c3d4\", \" \u{b7} x\\\"y\"}"),
            "{script}"
        );
        assert!(script.contains("if winName ends with (contents of s) then set keep to true"));
        assert!(script.contains("if not keep then set end of closeIds to (contents of i)"));
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

    #[test]
    fn placement_failure_leaves_windows_open_with_the_error() {
        let titles = vec!["a".to_string(), "b".to_string()];
        let done = outcome_after_placement(titles.clone(), Ok(()));
        assert!(done.tiled && done.note.is_none() && done.opened == 2);
        let failed = outcome_after_placement(titles.clone(), Err("boom (-25211)".into()));
        assert!(!failed.tiled);
        assert_eq!(failed.titles, titles);
        let note = failed.note.unwrap();
        assert!(
            note.contains("boom (-25211)") && note.contains("partly placed"),
            "{note}"
        );
    }

    #[test]
    fn the_wait_grows_with_the_window_count() {
        assert!(wait_for(9) > wait_for(1));
        assert!(wait_for(1) > Duration::from_secs(15));
    }

    #[test]
    fn open_script_closes_what_it_created_and_reraises() {
        let script = open_windows_script(&[surface("t", "c")]);
        assert!(script.contains("set createdIds to {}"));
        assert!(script.contains("set end of createdIds to id of win"));
        let handler = script
            .split("on error errMsg number errNum")
            .nth(1)
            .expect("handler");
        assert!(handler.contains("repeat with i in createdIds"));
        assert!(handler.contains("close window (first window whose id is (contents of i))"));
        assert!(handler.contains("error errMsg number errNum"));
        assert!(
            !script.contains("whose name"),
            "cleanup must never match by title"
        );
        let created = script.find("set end of createdIds").unwrap();
        let named = script.find("set name override").unwrap();
        assert!(
            created < named,
            "the id is kept before anything else can fail"
        );
    }

    #[test]
    fn open_failure_notes_say_what_was_cleaned_up() {
        let reported = open_failure_note("execution error: x (-1712)");
        assert!(reported.contains("windows it had opened were closed where possible"));
        let killed = open_failure_note(&format!("{KILLED_PREFIX} 25s"));
        assert!(killed.contains("some windows may still be open"));
        assert!(killed.contains("Close breakout windows"));
    }
}
