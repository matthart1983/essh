//! Screen 4 — the Host Monitor, and screen 5's narrow essentials column.
//!
//! Rebuilt to the design handoff. The layout is three headline boxes across
//! the top, then DISK beside VS PEERS, then PROCESSES:
//!
//! ```text
//! ┌ CPU ────────┐┌ MEMORY ─────┐┌ NETWORK ────┐
//! │ 1.4 %       ││ 15.0 GB/16  ││ 111 KB/s ↓  │
//! │ peers 31%…  ││ peers 11.2… ││ ↑3 · rtt…   │
//! │ ▁▂▃▄▅▃▂     ││ ▅▆▇▆▅▆▇     ││ ▂▃▂▄▃▂      │
//! └─────────────┘└─────────────┘└─────────────┘
//! ┌ DISK ──────────────────┐┌ VS PEERS ──────┐
//! ```
//!
//! Two rules from the handoff run through all of it:
//!
//! * **Every headline carries its peer median.** "23%" is not a finding;
//!   "23% against a fleet median of 31%" is.
//! * **Nothing is drawn for data we do not have.** A metric that cannot be
//!   read says *uncollected — why*, in words, and draws no bar and no curve.

use crate::design as d;
use crate::format::truncate_left;
use crate::monitor::history::MetricHistory;
use crate::monitor::{HostMetrics, MetricState};
use crate::theme::Theme;
use crate::tui::widgets;
use ratatui::{
    prelude::*,
    widgets::{Cell, Paragraph, Row, Table},
};

pub enum ProcessSort {
    Cpu,
    Memory,
}

/// Peer context for a metric, when a peer set exists.
#[derive(Clone, Debug, Default)]
pub struct PeerContext {
    pub cpu_median_pct: Option<f64>,
    pub mem_median_gb: Option<f64>,
    pub peers: usize,
}

/// One line of words explaining why a metric is absent.
fn explanation(state: &MetricState) -> String {
    state.explain().unwrap_or_else(|| "unavailable".to_string())
}

/// A headline box: big value, unit, sub-line with peer context, sparkline.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn headline_box(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    key: &str,
    label: &str,
    right: Option<&str>,
    value: Option<(String, String)>,
    sub: String,
    state: &MetricState,
    history: &[u64],
    color: Color,
) {
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            // Side panes pass an empty key: they are not addressable, and a
            // `┤├` badge with nothing in it is worse than no badge.
            key: (!key.is_empty()).then_some(key),
            title: Some(label),
            right,
            ..Default::default()
        },
    );
    if inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    match value {
        Some((v, unit)) => {
            lines.push(Line::from(vec![
                Span::styled(v, Style::default().fg(d::WHITE).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(unit, Style::default().fg(d::DIM)),
            ]));
            lines.push(Line::from(Span::styled(sub, Style::default().fg(d::FAINT))));
            if inner.height > 2 && !history.is_empty() {
                let ceiling = d::axis_ceiling(history);
                lines.push(Line::from(Span::styled(
                    d::sparkline(history, inner.width as usize, ceiling),
                    Style::default().fg(color),
                )));
            }
        }
        // No value: say why, in words, and draw no curve.
        None => lines.push(d::none_line(&explanation(state))),
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Screen 4, full width.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    cpu_history: &MetricHistory,
    mem_history: &MetricHistory,
    net_rx_history: &MetricHistory,
    net_tx_history: &MetricHistory,
    sort: &ProcessSort,
    process_scroll: usize,
    peers: &PeerContext,
    theme: &Theme,
) {
    d::paint_bg(f, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // headline boxes
            Constraint::Length(8), // disk + vs peers
            Constraint::Min(4),    // processes
        ])
        .split(area);

    // ── Headlines
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(rows[0]);

    let cpu = metrics.cpu_percent_opt();
    let cpu_sub = match (cpu, peers.cpu_median_pct) {
        (Some(_), Some(m)) => format!("peers median {:.0}% · load {:.2}", m, metrics.load_1m),
        (Some(_), None) if metrics.status.load.is_collected() => {
            format!("load {:.2} {:.2} {:.2}", metrics.load_1m, metrics.load_5m, metrics.load_15m)
        }
        _ => String::new(),
    };
    headline_box(
        f,
        cols[0],
        theme,
        "1",
        "cpu",
        None,
        cpu.map(|c| (format!("{:.1}", c), "%".into())),
        cpu_sub,
        &metrics.status.cpu,
        &cpu_history.as_slice_vec(),
        d::magnitude(cpu.unwrap_or(0.0)),
    );

    let mem_pct = metrics.mem_percent();
    let mem_value = mem_pct.map(|_| {
        (
            widgets::format_kb(metrics.mem_used_kb),
            format!("/ {}", widgets::format_kb(metrics.mem_total_kb)),
        )
    });
    let mem_sub = {
        let swap = if metrics.mem_swap_total_kb == 0 {
            "swap disabled".to_string()
        } else {
            format!("swap {}", widgets::format_kb(metrics.mem_swap_used_kb))
        };
        match (mem_pct, peers.mem_median_gb) {
            (Some(p), Some(m)) => format!("peers median {:.1} GB · {} · {:.0}% used", m, swap, p),
            (Some(p), None) => format!("{} · {:.0}% used", swap, p),
            _ => String::new(),
        }
    };
    headline_box(
        f,
        cols[1],
        theme,
        "2",
        "memory",
        None,
        mem_value,
        mem_sub,
        &metrics.status.mem,
        &mem_history.as_slice_vec(),
        d::ramp_at(&d::RAMP_NET, mem_pct.unwrap_or(0.0) / 100.0),
    );

    let net = metrics.net_opt();
    let net_sub = match net {
        Some((_, tx)) => format!("↑ {}", widgets::format_bytes_rate(tx)),
        None => String::new(),
    };
    headline_box(
        f,
        cols[2],
        theme,
        "3",
        "network",
        None,
        net.map(|(rx, _)| (widgets::format_bytes_rate(rx), "↓".into())),
        net_sub,
        &metrics.status.net,
        &net_rx_history.as_slice_vec(),
        d::VIOLET,
    );
    let _ = net_tx_history;

    // ── Disk + vs peers
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(38)])
        .split(rows[1]);
    render_disk(f, mid[0], metrics, theme);
    render_vs_peers(f, mid[1], metrics, peers, theme);

    // ── Processes
    render_processes(f, rows[2], metrics, sort, process_scroll, theme);
}

fn render_disk(f: &mut Frame, area: Rect, metrics: &HostMetrics, theme: &Theme) {
    let hidden = metrics.hidden_disk_count();
    let right = if hidden > 0 {
        format!("user volumes · {} hidden", hidden)
    } else {
        "user volumes · fullest first".to_string()
    };
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("4"),
            title: Some("disk"),
            right: Some(&right),
            ..Default::default()
        },
    );

    if !metrics.status.disk.is_collected() {
        f.render_widget(
            Paragraph::new(d::none_line(&explanation(&metrics.status.disk))),
            inner,
        );
        return;
    }

    let disks = metrics.user_disks();
    let header = Row::new(vec![
        Cell::from(d::header_cell("mount", false)),
        Cell::from(Line::from(d::header_cell("used", false)).right_aligned()),
        Cell::from(Line::from(d::header_cell("avail", false)).right_aligned()),
        Cell::from(d::header_cell("use", false)),
        Cell::from(Line::from(d::header_cell("%", false)).right_aligned()),
    ]);

    let meter_width = 12usize;
    let body: Vec<Row> = disks
        .iter()
        .take(inner.height.saturating_sub(2) as usize)
        .map(|disk| {
            let avail = disk.total_bytes.saturating_sub(disk.used_bytes);
            let (fill, track) = d::meter(disk.use_pct / 100.0, meter_width);
            let col = d::bounded_bad(disk.use_pct);
            Row::new(vec![
                Cell::from(truncate_left(&disk.mount, 24)).style(Style::default().fg(d::FG)),
                Cell::from(Line::from(widgets::format_bytes(disk.used_bytes)).right_aligned())
                    .style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(widgets::format_bytes(avail)).right_aligned())
                    .style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(vec![
                    Span::styled(fill, Style::default().fg(col)),
                    Span::styled(track, Style::default().fg(d::RULE)),
                ])),
                Cell::from(Line::from(format!("{:.0}%", disk.use_pct)).right_aligned())
                    .style(Style::default().fg(d::FG)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(16),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(meter_width as u16),
        Constraint::Length(5),
    ];
    f.render_widget(Table::new(body, widths).header(header), inner);

    // The footnote: hiding data silently is its own dishonesty.
    if hidden > 0 && inner.height > 2 {
        let note = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(d::none_line(&format!(
                "{} system volumes hidden — none over 1 GB or 90% full",
                hidden
            ))),
            note,
        );
    }
}

fn render_vs_peers(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    peers: &PeerContext,
    theme: &Theme,
) {
    let right = if peers.peers > 0 {
        format!("{} hosts", peers.peers)
    } else {
        String::new()
    };
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("5"),
            title: Some("vs peers"),
            sub: (!right.is_empty()).then_some(&right),
            foot_left: &[("D", " detail")],
            ..Default::default()
        },
    );

    if peers.peers == 0 {
        f.render_widget(
            Paragraph::new(d::none_line("no peer set — tag two hosts alike")),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut row = |label: &str, value: String, note: String, severity: f64| {
        let col = if severity > 0.0 {
            d::divergence(severity)
        } else {
            d::GREEN
        };
        let pad = 22usize
            .saturating_sub(label.chars().count() + value.chars().count() + 2)
            .max(1);
        lines.push(Line::from(vec![
            d::dot(col),
            Span::raw(" "),
            Span::styled(format!("{:<9}", label), Style::default().fg(d::DIM)),
            Span::styled(value, Style::default().fg(col)),
            Span::raw(" ".repeat(pad)),
            Span::styled(note, Style::default().fg(d::FAINT)),
        ]));
    };

    if let Some(cpu) = metrics.cpu_percent_opt() {
        if let Some(m) = peers.cpu_median_pct {
            let sev = ((cpu - m).abs() / 100.0).min(1.0);
            row("cpu", format!("{:.0}%", cpu), format!("median {:.0}%", m), sev);
        }
    }
    if let Some(disk) = metrics.user_disks().first() {
        row(
            "disk /",
            format!("{:.0}%", disk.use_pct),
            String::new(),
            if disk.use_pct > 85.0 { 0.8 } else { 0.0 },
        );
    }
    if let Some(up) = metrics.uptime_opt() {
        row("uptime", widgets::format_uptime(up), String::new(), 0.0);
    }

    if lines.is_empty() {
        lines.push(d::none_line("nothing collected yet"));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_processes(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    sort: &ProcessSort,
    scroll: usize,
    theme: &Theme,
) {
    let procs = match sort {
        ProcessSort::Cpu => &metrics.top_procs_cpu,
        ProcessSort::Memory => &metrics.top_procs_mem,
    };
    let right = match sort {
        ProcessSort::Cpu => "by cpu",
        ProcessSort::Memory => "by mem",
    };
    let count = format!("{} procs", procs.len());
    // Rows visible after the header line, so the range in the border matches
    // what is actually on screen rather than the window height.
    let visible = area.height.saturating_sub(3) as usize;
    let page = (procs.len() > visible && visible > 0).then(|| {
        format!(
            "{}–{} of {}",
            scroll + 1,
            (scroll + visible).min(procs.len()),
            procs.len()
        )
    });
    let inner = d::pane(
        f,
        area,
        theme,
        &d::PanelOpts {
            key: Some("6"),
            title: Some("processes"),
            sub: Some(&count),
            right: Some(right),
            // Binds live on the rule rather than costing a footer row. The
            // way *out* comes first: a full-screen view with no visible exit
            // is how a user concludes the app has hung.
            foot_left: &[
                ("⎋", " terminal"),
                ("↑↓", " scroll"),
                ("s", " sort"),
                ("?", " help"),
            ],
            foot_right: page.as_deref(),
            ..Default::default()
        },
    );

    if !metrics.status.procs.is_collected() {
        f.render_widget(
            Paragraph::new(d::none_line(&explanation(&metrics.status.procs))),
            inner,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(Line::from(d::header_cell("pid", false)).right_aligned()),
        Cell::from(d::header_cell("command", false)),
        Cell::from(Line::from(d::header_cell("cpu%", false)).right_aligned()),
        Cell::from(Line::from(d::header_cell("mem%", false)).right_aligned()),
        Cell::from(Line::from(d::header_cell("rss", false)).right_aligned()),
    ]);

    let name_width = (inner.width as usize).saturating_sub(8 + 7 + 7 + 11).max(16);
    let body: Vec<Row> = procs
        .iter()
        .skip(scroll)
        .take(inner.height.saturating_sub(1) as usize)
        .map(|p| {
            Row::new(vec![
                Cell::from(Line::from(p.pid.to_string()).right_aligned())
                    .style(Style::default().fg(d::FAINT)),
                Cell::from(truncate_left(&p.name, name_width)).style(Style::default().fg(d::FG)),
                Cell::from(Line::from(format!("{:.1}", p.cpu_pct)).right_aligned()).style(
                    Style::default().fg(if p.cpu_pct > 2.0 { d::AMBER } else { d::FG }),
                ),
                Cell::from(Line::from(format!("{:.1}", p.mem_pct)).right_aligned())
                    .style(Style::default().fg(d::DIM)),
                Cell::from(Line::from(widgets::format_kb(p.mem_rss_kb)).right_aligned())
                    .style(Style::default().fg(d::DIM)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(16),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(10),
    ];
    f.render_widget(Table::new(body, widths).header(header), inner);
}

/// Screen 5's right-hand column: essentials only.
///
/// *"the narrow pane drops to essentials: cpu, mem, the fullest mount, net,
/// top procs. A split is not a smaller copy of the full view."*
pub fn render_essentials(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    cpu_history: Option<&MetricHistory>,
    mem_history: Option<&MetricHistory>,
    net_history: Option<&MetricHistory>,
    peers: &PeerContext,
    rtt_ms: Option<f64>,
    theme: &Theme,
) {
    d::paint_bg(f, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        // Five rows, not four: border, value, sub, sparkline, border. At four
        // the sparkline fell outside the box and `headline_box` skipped it
        // silently — the mini pane had no graphs at all, and the disk meter
        // was clipped off the same way.
        .constraints([
            Constraint::Length(5), // cpu
            Constraint::Length(5), // memory
            Constraint::Length(4), // disk: mount line + meter
            Constraint::Length(5), // net
            Constraint::Min(3),    // top
        ])
        .split(area);

    let empty = MetricHistory::new(60);
    // The mockup puts context in each box's right-hand slot rather than
    // spending a line on it: `peers 31%`, `94%`, `rtt 2.1ms`.
    let peers_note = peers
        .cpu_median_pct
        .map(|m| format!("peers {:.0}%", m));
    let mem_right = metrics.mem_percent().map(|p| format!("{:.0}%", p));
    // Only when measured. The v1 monitor claimed "Excellent" against an
    // unmeasured RTT; a missing number says nothing rather than something
    // false.
    let rtt_note = rtt_ms.map(|r| format!("rtt {r:.1}ms"));

    let cpu = metrics.cpu_percent_opt();
    headline_box(
        f,
        rows[0],
        theme,
        "",
        "cpu",
        peers_note.as_deref(),
        cpu.map(|c| (format!("{:.1}", c), format!("% · load {:.2}", metrics.load_1m))),
        String::new(),
        &metrics.status.cpu,
        &cpu_history.unwrap_or(&empty).as_slice_vec(),
        d::magnitude(cpu.unwrap_or(0.0)),
    );

    let mem_pct = metrics.mem_percent();
    headline_box(
        f,
        rows[1],
        theme,
        "",
        "memory",
        mem_right.as_deref(),
        mem_pct.map(|_| {
            (
                widgets::format_kb(metrics.mem_used_kb),
                format!("/ {}", widgets::format_kb(metrics.mem_total_kb)),
            )
        }),
        String::new(),
        &metrics.status.mem,
        &mem_history.unwrap_or(&empty).as_slice_vec(),
        d::ramp_at(&d::RAMP_NET, mem_pct.unwrap_or(0.0) / 100.0),
    );

    // Disk: the fullest volume only.
    let inner = d::pane(
        f,
        rows[2],
        theme,
        &d::PanelOpts {
            title: Some("disk"),
            right: Some("fullest volume"),
            ..Default::default()
        },
    );
    match metrics.user_disks().first() {
        Some(disk) => {
            let (fill, track) = d::meter(disk.use_pct / 100.0, inner.width.saturating_sub(6) as usize);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            truncate_left(&disk.mount, inner.width.saturating_sub(6) as usize),
                            Style::default().fg(d::DIM),
                        ),
                        Span::raw(" "),
                        Span::styled(
                            format!("{:.0}%", disk.use_pct),
                            Style::default().fg(d::FG),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(fill, Style::default().fg(d::bounded_bad(disk.use_pct))),
                        Span::styled(track, Style::default().fg(d::RULE)),
                    ]),
                ]),
                inner,
            );
        }
        None => f.render_widget(
            Paragraph::new(d::none_line(&explanation(&metrics.status.disk))),
            inner,
        ),
    }

    let net = metrics.net_opt();
    headline_box(
        f,
        rows[3],
        theme,
        "",
        "net",
        rtt_note.as_deref(),
        net.map(|(rx, _)| (widgets::format_bytes_rate(rx), "↓".into())),
        net.map(|(_, tx)| format!("↑ {}", widgets::format_bytes_rate(tx)))
            .unwrap_or_default(),
        &metrics.status.net,
        &net_history.unwrap_or(&empty).as_slice_vec(),
        d::VIOLET,
    );

    // Top processes, name + cpu + rss only.
    let inner = d::pane(
        f,
        rows[4],
        theme,
        &d::PanelOpts {
            title: Some("top"),
            right: Some("by cpu"),
            ..Default::default()
        },
    );
    if metrics.status.procs.is_collected() {
        // command · cpu% · rss, as the mockup has it. Dropping RSS made the
        // pane say which process is busy but not which is heavy — and memory
        // is the thing you are usually hunting when you split the view.
        let rss_w = 9usize;
        let cpu_w = 6usize;
        let name_width = (inner.width as usize)
            .saturating_sub(rss_w + cpu_w)
            .max(10);
        let lines: Vec<Line> = metrics
            .top_procs_cpu
            .iter()
            .take(inner.height as usize)
            .map(|p| {
                Line::from(vec![
                    Span::styled(
                        format!("{:<w$}", truncate_left(&p.name, name_width), w = name_width),
                        Style::default().fg(d::FG),
                    ),
                    Span::styled(
                        format!("{:>w$.1}", p.cpu_pct, w = cpu_w),
                        Style::default().fg(if p.cpu_pct > 2.0 { d::AMBER } else { d::FG }),
                    ),
                    Span::styled(
                        format!("{:>w$}", widgets::format_kb(p.mem_rss_kb), w = rss_w),
                        Style::default().fg(d::DIM),
                    ),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        f.render_widget(
            Paragraph::new(d::none_line(&explanation(&metrics.status.procs))),
            inner,
        );
    }
}

#[cfg(test)]
mod essentials_tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn populated() -> (HostMetrics, MetricHistory) {
        let mut m = HostMetrics {
            cpu_percent: 1.4,
            load_1m: 1.37,
            mem_total_kb: 16 * 1024 * 1024,
            mem_used_kb: 15 * 1024 * 1024,
            net_rx_bps: 111_000.0,
            net_tx_bps: 3_000.0,
            disks: vec![crate::monitor::DiskInfo {
                mount: "/System/Volumes/Data".into(),
                total_bytes: 500 * 1024 * 1024 * 1024,
                used_bytes: 210 * 1024 * 1024 * 1024,
                use_pct: 42.0,
            }],
            ..Default::default()
        };
        m.top_procs_cpu = vec![crate::monitor::ProcessInfo {
            pid: 1,
            name: "/usr/sbin/nginx".into(),
            cpu_pct: 2.8,
            mem_pct: 1.0,
            mem_rss_kb: 118_800,
            state: "S".into(),
        }];
        for slot in [
            &mut m.status.cpu,
            &mut m.status.mem,
            &mut m.status.load,
            &mut m.status.disk,
            &mut m.status.net,
            &mut m.status.procs,
        ] {
            *slot = crate::monitor::MetricState::Collected;
        }
        let mut h = MetricHistory::new(60);
        for i in 0..60 {
            h.push((20.0 + ((i as f64) * 0.4).sin() * 15.0) as u64);
        }
        (m, h)
    }

    fn screen(width: u16, height: u16) -> String {
        let (m, h) = populated();
        let theme = crate::theme::dark();
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| {
            render_essentials(
                f,
                f.area(),
                &m,
                Some(&h),
                Some(&h),
                Some(&h),
                &PeerContext {
                    cpu_median_pct: Some(31.0),
                    mem_median_gb: Some(11.2),
                    peers: 39,
                },
                Some(2.1),
                &theme,
            );
        })
        .unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The mini pane must draw its graphs.
    ///
    /// The panels were laid out one row too short, so `headline_box` found no
    /// room for the sparkline and skipped it — silently. The split view had no
    /// graphs at all and nothing said why.
    #[test]
    fn the_mini_pane_draws_its_sparklines_and_meter() {
        let s = screen(44, 34);
        let braille = s.chars().filter(|c| ('\u{2800}'..='\u{28ff}').contains(c)).count();
        assert!(
            braille > 20,
            "no sparklines in the mini pane — the boxes are too short again:\n{s}"
        );
        assert!(s.contains('█'), "the disk meter is missing:\n{s}");
    }

    /// The mockup puts context in each box's right-hand slot.
    #[test]
    fn the_mini_pane_carries_its_context_labels() {
        let s = screen(44, 34);
        assert!(s.contains("peers 31%"), "no peer median on cpu:\n{s}");
        assert!(s.contains("rtt 2.1ms"), "no rtt on net:\n{s}");
        assert!(s.contains("fullest volume"), "no disk qualifier:\n{s}");
    }

    /// command · cpu · rss, so the pane says which process is heavy as well
    /// as which is busy.
    #[test]
    fn the_top_table_shows_memory_as_well_as_cpu() {
        let s = screen(44, 34);
        assert!(s.contains("nginx"), "no process listed:\n{s}");
        assert!(s.contains("MB"), "no RSS column:\n{s}");
    }
}

#[cfg(test)]
mod layout_dump {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    /// Not an assertion — a diagnostic. `cargo test dump_monitor -- --ignored
    /// --nocapture` prints the rendered screen so layout bugs can be read
    /// directly instead of inferred from a screenshot.
    #[test]
    #[ignore]
    fn dump_monitor() {
        let mut term = Terminal::new(TestBackend::new(176, 27)).unwrap();
        let mut m = HostMetrics::default();
        m.cpu_percent = 0.1;
        m.load_1m = 0.02;
        m.mem_total_kb = 9_400_000;
        m.mem_used_kb = 671_900;
        m.net_rx_bps = 1000.0;
        m.net_tx_bps = 3400.0;
        for slot in [
            &mut m.status.cpu,
            &mut m.status.mem,
            &mut m.status.load,
            &mut m.status.net,
        ] {
            *slot = crate::monitor::MetricState::Collected;
        }
        let t = crate::theme::dark();
        let mut h = MetricHistory::new(60);
        for i in 0..60 {
            h.push((10.0 + ((i as f64) * 0.4).sin() * 8.0) as u64);
        }
        term.draw(|f| {
            render(
                f,
                f.area(),
                &m,
                &h,
                &h,
                &h,
                &h,
                &ProcessSort::Cpu,
                0,
                &PeerContext::default(),
                &t,
            );
        })
        .unwrap();
        let b = term.backend().buffer();
        for y in 0..27 {
            let row: String = (0..176)
                .map(|x| b[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect();
            println!("{y:2}|{row}|");
        }
    }
}
