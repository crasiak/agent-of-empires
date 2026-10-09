//! Bounded, session-attributed resource history for the info panel.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::process::metrics::MetricsSnapshot;
use crate::tui::styles::Theme;

const HISTORY_SAMPLES: usize = 60;
const STALE_AFTER: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Default)]
struct Sample {
    cpu: Option<f64>,
    memory: Option<u64>,
}

#[derive(Default)]
pub(crate) struct ResourceHistory {
    sampled_at: Option<Instant>,
    sessions: HashMap<String, VecDeque<Sample>>,
}

impl ResourceHistory {
    pub(crate) fn record(&mut self, snapshot: &MetricsSnapshot, now: Instant) {
        if self
            .sampled_at
            .is_some_and(|at| now.duration_since(at) > STALE_AFTER)
        {
            self.sessions.clear();
        }
        self.sampled_at = Some(now);
        let agents: HashMap<_, _> = snapshot
            .agents
            .iter()
            .map(|agent| (agent.id.as_str(), agent))
            .collect();
        for agent in &snapshot.agents {
            self.sessions.entry(agent.id.clone()).or_default();
        }
        for (id, samples) in &mut self.sessions {
            if samples.len() == HISTORY_SAMPLES {
                samples.pop_front();
            }
            let agent = agents.get(id.as_str());
            samples.push_back(Sample {
                cpu: agent
                    .and_then(|agent| agent.cpu_fraction)
                    .filter(|v| v.is_finite() && *v >= 0.0),
                memory: agent.and_then(|agent| agent.rss_bytes),
            });
        }
        self.sessions.retain(|id, samples| {
            agents.contains_key(id.as_str())
                || samples
                    .iter()
                    .any(|sample| sample.cpu.is_some() || sample.memory.is_some())
        });
    }

    fn samples(&self, id: &str, now: Instant) -> Option<&VecDeque<Sample>> {
        self.sampled_at
            .filter(|at| now.duration_since(*at) <= STALE_AFTER)?;
        self.sessions.get(id)
    }

    pub(crate) fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        instance: &crate::session::Instance,
        theme: &Theme,
    ) {
        let active = !instance.is_archived()
            && !instance.is_trashed()
            && !instance.is_snoozed()
            && !instance.is_shown_dormant()
            && matches!(
                instance.status,
                crate::session::Status::Running
                    | crate::session::Status::Waiting
                    | crate::session::Status::Idle
            );
        let sandboxed = instance.is_sandboxed();
        let samples = active
            .then(|| self.samples(&instance.id, Instant::now()))
            .flatten();
        let current = samples.and_then(|s| s.back()).copied().unwrap_or_default();
        let cpu = current
            .cpu
            .map_or_else(|| "?".to_string(), |v| format!("{:.1}%", v * 100.0));
        let memory = current
            .memory
            .map_or_else(|| "?".to_string(), super::diagnostics::format_bytes);
        let cpu_values: Vec<_> = samples.into_iter().flatten().map(|s| s.cpu).collect();
        let memory_values: Vec<_> = samples
            .into_iter()
            .flatten()
            .map(|s| s.memory.map(|v| v as f64))
            .collect();
        let memory_max = memory_values.iter().flatten().copied().fold(1.0, f64::max);
        let heading = if sandboxed {
            "Container · CPU % host / Mem auto"
        } else {
            "Agent tree · CPU % host / RSS auto"
        };
        let lines = vec![
            Line::styled(heading, Style::default().fg(theme.dimmed)),
            resource_line(
                "CPU",
                &cpu,
                &cpu_values,
                1.0,
                area.width,
                theme.accent,
                theme,
            ),
            resource_line(
                if sandboxed { "Mem" } else { "RSS" },
                &memory,
                &memory_values,
                memory_max,
                area.width,
                theme.running,
                theme,
            ),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn resource_line(
    label: &str,
    value: &str,
    samples: &[Option<f64>],
    max: f64,
    width: u16,
    color: Color,
    theme: &Theme,
) -> Line<'static> {
    let prefix = format!("{label} {value:>6} ");
    let columns = (width as usize)
        .saturating_sub(prefix.len())
        .min(HISTORY_SAMPLES);
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(theme.text)),
        Span::styled(sparkline(samples, max, columns), Style::default().fg(color)),
    ])
}

fn sparkline(samples: &[Option<f64>], max: f64, columns: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let visible = &samples[samples.len().saturating_sub(columns)..];
    let mut result = " ".repeat(columns.saturating_sub(visible.len()));
    result.extend(visible.iter().map(|value| match value {
        Some(value) => BARS[((value / max).clamp(0.0, 1.0) * 7.0).round() as usize],
        None => '·',
    }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::metrics::AgentMetric;

    fn snapshot(rows: &[(&str, Option<f64>, Option<u64>)]) -> MetricsSnapshot {
        MetricsSnapshot {
            agents: rows
                .iter()
                .map(|(id, cpu, memory)| AgentMetric {
                    id: id.to_string(),
                    cpu_fraction: *cpu,
                    rss_bytes: *memory,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn histories_are_bounded_attributed_and_expire() {
        let start = Instant::now();
        let mut history = ResourceHistory::default();
        for n in 0..65 {
            history.record(
                &snapshot(&[("a", Some(0.25), Some(n)), ("b", Some(0.75), Some(999))]),
                start + Duration::from_secs(n),
            );
        }
        let now = start + Duration::from_secs(64);
        let a = history.samples("a", now).unwrap();
        assert_eq!(a.len(), 60);
        assert_eq!(a.front().unwrap().memory, Some(5));
        assert_eq!(a.back().unwrap().cpu, Some(0.25));
        assert_eq!(
            history.samples("b", now).unwrap().back().unwrap().memory,
            Some(999)
        );
        assert!(history.samples("other", now).is_none());
        assert!(history
            .samples("a", now + STALE_AFTER + Duration::from_secs(1))
            .is_none());

        history.record(
            &snapshot(&[("a", None, None)]),
            now + Duration::from_secs(1),
        );
        let b = history.samples("b", now + Duration::from_secs(1)).unwrap();
        assert!(b.back().unwrap().cpu.is_none());
        assert_eq!(b[b.len() - 2].cpu, Some(0.75));
        let values: Vec<_> = b.iter().map(|sample| sample.cpu).collect();
        assert_eq!(sparkline(&values, 1.0, 2), "▆·");
        assert!(history
            .samples("a", now + Duration::from_secs(1))
            .unwrap()
            .back()
            .unwrap()
            .cpu
            .is_none());
        history.record(
            &snapshot(&[("b", Some(0.5), Some(500))]),
            now + Duration::from_secs(2),
        );
        let b = history.samples("b", now + Duration::from_secs(2)).unwrap();
        assert_eq!(b[b.len() - 3].memory, Some(999));
        assert!(b[b.len() - 2].memory.is_none());
        assert_eq!(b.back().unwrap().memory, Some(500));
        history.record(
            &snapshot(&[("a", Some(0.0), Some(0))]),
            now + Duration::from_secs(20),
        );
        assert_eq!(
            history
                .samples("a", now + Duration::from_secs(20))
                .unwrap()
                .len(),
            1
        );
        for n in 21..=80 {
            history.record(&MetricsSnapshot::default(), now + Duration::from_secs(n));
        }
        assert!(history.sessions.is_empty());
    }

    #[test]
    fn stale_or_stopped_sessions_do_not_show_live_readings() {
        use ratatui::backend::TestBackend;
        let mut instance = crate::session::Instance::new("selected", "/tmp");
        for (active, age, known) in [(true, 0, true), (true, 6, false), (false, 0, false)] {
            instance.status = if active {
                crate::session::Status::Running
            } else {
                crate::session::Status::Stopped
            };
            let mut history = ResourceHistory::default();
            history.record(
                &snapshot(&[(&instance.id, Some(0.125), Some(128 << 20))]),
                Instant::now() - Duration::from_secs(age),
            );
            let mut terminal = ratatui::Terminal::new(TestBackend::new(40, 3)).unwrap();
            terminal
                .draw(|f| history.render(f, f.area(), &instance, &Theme::default()))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert_eq!(text.contains("12.5%"), known, "{text}");
            assert_eq!(text.contains("128M"), known, "{text}");
            assert_eq!(text.contains('?'), !known, "{text}");
        }
    }

    #[test]
    fn graph_distinguishes_unknown_zero_and_scale() {
        assert_eq!(
            sparkline(&[Some(0.0), None, Some(0.5), Some(1.0)], 1.0, 5),
            " ▁·▅█"
        );
        assert_eq!(
            sparkline(&[Some(0.0), Some(20.0), Some(40.0)], 40.0, 2),
            "▅█"
        );
        assert_eq!(sparkline(&[Some(1.0)], 1.0, 0), "");
    }
}
