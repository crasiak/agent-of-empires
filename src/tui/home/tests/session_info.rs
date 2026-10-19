use super::*;
use crate::process::metrics::{AgentMetric, MetricsSnapshot};
use crate::tui::styles::Theme;
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

fn render(view: &mut HomeView, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
    terminal
        .draw(|f| view.render(f, f.area(), &Theme::default(), None, None, None))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn rows(buffer: &Buffer, from: u16, to: u16) -> String {
    (from..to)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[serial]
fn sampling_follows_visible_session_info() {
    let mut env = create_test_env_with_sessions(1);
    let id = env.view.instance_at(0).id.clone();
    env.view.select_session_by_id(&id);
    env.view.show_diagnostics = false;
    env.view.system_health_tip_earned = true;
    env.view.show_preview_info = true;
    render(&mut env.view, 70);
    env.view.request_metrics_refresh();
    assert!(!env.view.pending_metrics_refresh);
    render(&mut env.view, 150);
    env.view.request_metrics_refresh();
    assert!(env.view.pending_metrics_refresh);
}

#[test]
#[serial]
fn session_info_contains_badges_and_only_selected_agent_resources() {
    let mut env = create_test_env_with_sessions(2);
    let first = env.view.instance_at(0).id.clone();
    let second = env.view.instance_at(1).id.clone();
    for id in [&first, &second] {
        env.view
            .mutate_instance(id, |inst| inst.status = Status::Running);
    }
    env.view.show_preview_info = true;
    env.view.show_usage_overlay = true;
    env.view.show_diagnostics = false;
    env.view.resource_history.record(
        &MetricsSnapshot {
            agents: vec![
                AgentMetric {
                    id: first.clone(),
                    cpu_fraction: Some(0.125),
                    rss_bytes: Some(128 << 20),
                    ..Default::default()
                },
                AgentMetric {
                    id: second.clone(),
                    cpu_fraction: Some(0.75),
                    rss_bytes: Some(999 << 20),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        std::time::Instant::now(),
    );
    env.view.select_session_by_id(&first);
    env.view.usage_summary = Some((
        first.clone(),
        crate::usage::UsageSummary {
            tracked: true,
            clears: 12,
            ..Default::default()
        },
    ));

    for mode in [
        ViewMode::Structured,
        ViewMode::Terminal,
        ViewMode::Tool("shell".into()),
    ] {
        env.view.view_mode = mode;
        let buffer = render(&mut env.view, 150);
        let output_y = env.view.preview_pane_area.y;
        let info = rows(&buffer, 0, output_y);
        assert!(info.contains("clr 12"), "{info}");
        assert!(info.contains("12.5%"), "{info}");
        assert!(info.contains("128M"), "{info}");
        assert!(!info.contains("75.0%"));
        let output = rows(&buffer, output_y, 38);
        assert!(!output.contains("clr 12"));
        assert!(!output.contains("12.5%"));
        assert_eq!(
            env.view.preview_visible_rows,
            env.view.preview_pane_area.height as usize
        );
    }

    env.view.show_usage_overlay = false;
    let buffer = render(&mut env.view, 150);
    let info = rows(&buffer, 0, env.view.preview_pane_area.y);
    assert!(!info.contains("clr 12"));
    assert!(info.contains("12.5%"));
    env.view.show_usage_overlay = true;

    env.view.select_session_by_id(&second);
    let buffer = render(&mut env.view, 150);
    let info = rows(&buffer, 0, env.view.preview_pane_area.y);
    assert!(info.contains("75.0%"), "{info}");
    assert!(!info.contains("12.5%"));
    assert!(!info.contains("clr 12"));

    env.view
        .mutate_instance(&second, |inst| inst.status = Status::Stopped);
    let buffer = render(&mut env.view, 150);
    let info = rows(&buffer, 0, env.view.preview_pane_area.y);
    assert!(!info.contains("75.0%"));
    assert!(!info.contains("999M"));
    env.view
        .mutate_instance(&second, |inst| inst.status = Status::Running);

    for (show_info, width) in [(false, 150), (true, 70)] {
        env.view.show_preview_info = show_info;
        let buffer = render(&mut env.view, width);
        let text = rows(&buffer, 0, 38);
        assert!(!text.contains("75.0%"));
        assert!(!text.contains("clr 12"));
    }
}
