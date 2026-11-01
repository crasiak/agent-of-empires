//! TOML-defined menu formats: recognisers that turn text (agent output, bash prompts, panes) into a `Menu`.
use super::menu::{InputOption, Kind, Menu, MenuInput, RoleWords, Source};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const WINDOW: usize = 12;

pub const BUILTIN: &[(&str, &str)] = &[
    (
        "text.bracket-shortkey.toml",
        include_str!("../../../../assets/afk/menu-formats/text.bracket-shortkey.toml"),
    ),
    (
        "text.numbered-list.toml",
        include_str!("../../../../assets/afk/menu-formats/text.numbered-list.toml"),
    ),
    (
        "text.yes-no.toml",
        include_str!("../../../../assets/afk/menu-formats/text.yes-no.toml"),
    ),
    (
        "text.select-prompt.toml",
        include_str!("../../../../assets/afk/menu-formats/text.select-prompt.toml"),
    ),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixedOption {
    key: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatFile {
    id: String,
    sources: Vec<Source>,
    kind: Kind,
    #[serde(default)]
    trigger: Option<String>,
    #[serde(default)]
    option_pattern: Option<String>,
    #[serde(default)]
    footer_pattern: Option<String>,
    #[serde(default)]
    options_after_trigger: bool,
    #[serde(default = "two")]
    min_options: usize,
    #[serde(default = "three")]
    question_lines_above: usize,
    #[serde(default)]
    label_includes_key: bool,
    #[serde(default, rename = "option")]
    fixed: Vec<FixedOption>,
    #[serde(default)]
    roles: Option<RoleWords>,
}

fn two() -> usize {
    2
}

fn three() -> usize {
    3
}

struct Format {
    id: String,
    sources: Vec<Source>,
    kind: Kind,
    trigger: Option<Regex>,
    option: Option<Regex>,
    footer: Option<Regex>,
    after_trigger: bool,
    min_options: usize,
    above: usize,
    label_includes_key: bool,
    fixed: Vec<FixedOption>,
    roles: RoleWords,
}

pub struct Formats(Vec<Format>);

pub fn user_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/aoe/afk/menu-formats"))
}

fn compile(name: &str, text: &str) -> Result<Format> {
    let f: FormatFile =
        toml::from_str(text).with_context(|| format!("{name}: invalid format file"))?;
    ensure!(!f.id.is_empty() && f.id.len() <= 128, "{name}: invalid id");
    ensure!(!f.sources.is_empty(), "{name}: no sources");
    ensure!(
        f.option_pattern.is_some() || (!f.fixed.is_empty() && f.trigger.is_some()),
        "{name}: needs option_pattern, or trigger and [[option]]"
    );
    ensure!(
        f.fixed.is_empty() || f.option_pattern.is_none(),
        "{name}: [[option]] and option_pattern are mutually exclusive"
    );
    ensure!(
        f.footer_pattern.is_none() || f.option_pattern.is_some(),
        "{name}: footer_pattern needs option_pattern"
    );
    let re = |p: &Option<String>| {
        p.as_deref()
            .map(Regex::new)
            .transpose()
            .with_context(|| format!("{name}: invalid regex"))
    };
    // role_for lowercases the label but not the words, so lowercase the extras here.
    let lower = |ws: Vec<String>| ws.into_iter().map(|w| w.to_lowercase());
    let mut roles = RoleWords::default();
    if let Some(extra) = f.roles {
        // An empty word would match every label.
        ensure!(
            [&extra.approve, &extra.deny, &extra.revise]
                .iter()
                .all(|ws| ws.iter().all(|w| !w.trim().is_empty())),
            "{name}: empty role word"
        );
        roles.approve.extend(lower(extra.approve));
        roles.deny.extend(lower(extra.deny));
        roles.revise.extend(lower(extra.revise));
    }
    let option = re(&f.option_pattern)?;
    ensure!(
        option
            .as_ref()
            .is_none_or(|o| o.capture_names().flatten().any(|n| n == "label")),
        "{name}: option_pattern needs a `label` group"
    );
    Ok(Format {
        trigger: re(&f.trigger)?,
        option,
        footer: re(&f.footer_pattern)?,
        id: f.id,
        sources: f.sources,
        kind: f.kind,
        after_trigger: f.options_after_trigger,
        min_options: f.min_options.max(1),
        above: f.question_lines_above,
        label_includes_key: f.label_includes_key,
        fixed: f.fixed,
        roles,
    })
}

impl Formats {
    pub fn from_sources(files: &[(String, String)]) -> (Formats, Vec<String>) {
        let mut out: Vec<Format> = Vec::new();
        let mut warnings = Vec::new();
        for (name, text) in files {
            match compile(name, text) {
                // Later files (user dir) replace earlier ones (built-ins) with the same id, in place.
                Ok(f) => match out.iter().position(|o| o.id == f.id) {
                    Some(i) => out[i] = f,
                    None => out.push(f),
                },
                Err(e) => warnings.push(format!("{e:#}")),
            }
        }
        (Formats(out), warnings)
    }

    pub fn load(user: Option<&Path>) -> (Formats, Vec<String>) {
        let mut files: Vec<(String, String)> = BUILTIN
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        let mut warnings = Vec::new();
        if let Some(dir) = user {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut paths: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    .collect();
                paths.sort();
                for p in paths.into_iter().take(64) {
                    match std::fs::read_to_string(&p) {
                        Ok(t) if t.len() <= 16 * 1024 => files.push((p.display().to_string(), t)),
                        Ok(_) => warnings.push(format!("{}: format file too large", p.display())),
                        Err(e) => warnings.push(format!("{}: {e}", p.display())),
                    }
                }
            }
        }
        let (formats, mut w) = Self::from_sources(&files);
        warnings.append(&mut w);
        (formats, warnings)
    }

    pub fn ids(&self) -> Vec<String> {
        self.0.iter().map(|f| f.id.clone()).collect()
    }

    /// First match wins, in load order. A recogniser that can't build a valid menu is no match.
    pub fn recognise(&self, source: Source, text: &str) -> Option<Menu> {
        // Fenced text is quoted data, never a live menu. So is a `>` blockquote in agent text; in a
        // prompt or pane `>` is a selection cursor and its row is an option.
        let mut fence: Option<&str> = None;
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| {
                let t = l.trim_start();
                let marker = ["```", "~~~"].into_iter().find(|m| t.starts_with(m));
                match (fence, marker) {
                    (None, Some(m)) => fence = Some(m),
                    (Some(open), Some(m)) if open == m => fence = None,
                    _ => {}
                }
                fence.is_none()
                    && marker.is_none()
                    && !t.is_empty()
                    && !(source == Source::AgentText && t.starts_with('>'))
            })
            .collect();
        let window = &lines[lines.len().saturating_sub(WINDOW)..];
        self.0
            .iter()
            .filter(|f| f.sources.contains(&source))
            .find_map(|f| f.matches(source, window, text))
    }
}

impl Format {
    fn matches(&self, source: Source, window: &[&str], raw: &str) -> Option<Menu> {
        let is_option = |l: &str| self.option.as_ref().is_some_and(|re| re.is_match(l));
        let Some(trigger) = &self.trigger else {
            return self.build(source, window, None, raw);
        };
        // A line that looks like an option is never the question. Formats that scan after the
        // trigger try each trigger line from the bottom up; others use only the last one.
        let mut candidates = (0..window.len())
            .rev()
            .filter(|&i| trigger.is_match(window[i]) && !is_option(window[i]));
        if self.after_trigger {
            candidates.find_map(|i| self.build(source, window, Some(i), raw))
        } else {
            self.build(source, window, Some(candidates.next()?), raw)
        }
    }

    fn build(
        &self,
        source: Source,
        window: &[&str],
        trigger_at: Option<usize>,
        raw: &str,
    ) -> Option<Menu> {
        let scan_from = if self.after_trigger {
            trigger_at.map_or(0, |i| i + 1)
        } else {
            0
        };
        let mut options = Vec::new();
        let mut first_option_line = None;
        if let Some(re) = &self.option {
            for (i, line) in window.iter().enumerate().skip(scan_from) {
                if Some(i) == trigger_at {
                    continue;
                }
                for c in re.captures_iter(line) {
                    let mut label = c.name("label")?.as_str().trim().to_string();
                    let key = match c.name("key") {
                        Some(k) => {
                            // "[A]pprove" needs its key back; "[A]Approve" already has it.
                            if self.label_includes_key
                                && !label.to_lowercase().starts_with(&k.as_str().to_lowercase())
                            {
                                label.insert_str(0, k.as_str());
                            }
                            k.as_str().to_string()
                        }
                        None => (options.len() + 1).to_string(),
                    };
                    first_option_line.get_or_insert(i);
                    options.push(InputOption {
                        key,
                        label,
                        role: None,
                    });
                }
            }
            if options.len() < self.min_options {
                return None;
            }
            // Options must be the end of the text: nothing but options (and footer lines) after the first option line.
            let is_footer = |l: &str| self.footer.as_ref().is_some_and(|f| f.is_match(l.trim()));
            if (self.trigger.is_none() || self.after_trigger)
                && first_option_line
                    .is_some_and(|i| window[i..].iter().any(|l| !re.is_match(l) && !is_footer(l)))
            {
                return None;
            }
        } else {
            options = self
                .fixed
                .iter()
                .map(|o| InputOption {
                    key: o.key.clone(),
                    label: o.label.clone(),
                    role: None,
                })
                .collect();
        }
        let question = match trigger_at {
            Some(i) => window[i].trim().to_string(),
            None => {
                let end = first_option_line?;
                window[end.saturating_sub(self.above)..end].join("\n")
            }
        };
        if question.trim().is_empty() {
            return None;
        }
        Menu::from_input(
            MenuInput {
                source,
                format: self.id.clone(),
                kind: self.kind,
                question,
                subject: None,
                options,
                free_text: false,
                warnings: vec![],
                deadline_ms: None,
                raw: raw.to_string(),
            },
            &self.roles,
        )
        .ok()
    }
}
