use crate::monitor::history::MetricHistory;
use crate::monitor::HostMetrics;
use crate::theme::Theme;
use crate::tui::widgets;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

pub enum ProcessSort {
    Cpu,
    Memory,
}

/// Render the host monitor overlay (replaces terminal when active)
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
    theme: &Theme,
) {
    // Outer border fills the full area
    let outer_block = Block::bordered()
        .title(" Host Monitor ")
        .title_style(Style::default().fg(theme.brand).bold())
        .border_style(Style::default().fg(theme.border));
    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    // Fixed section heights (content + bottom border for separators)
    let fixed_height: u16 = 3 + 3 + 2 + 2 + 1; // cpu+mem+load+net+footer
    let available = inner.height.saturating_sub(fixed_height);

    let disk_data = metrics.disks.len().min(15) as u16;
    let proc_data = match sort {
        ProcessSort::Cpu => metrics.top_procs_cpu.len(),
        ProcessSort::Memory => metrics.top_procs_mem.len(),
    }
    .min(15) as u16;
    let desired_disk = disk_data + 2; // +1 header +1 border

    let disk_height = desired_disk.min(available);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),           // CPU: 2 content + 1 border
            Constraint::Length(3),           // Memory: 2 content + 1 border
            Constraint::Length(2),           // Load: 1 content + 1 border
            Constraint::Length(disk_height), // Disk table (adaptive, includes border)
            Constraint::Length(2),           // Net: 1 content + 1 border
            Constraint::Min(proc_data + 1),  // Processes (fills remaining space)
            Constraint::Length(1),           // Footer
        ])
        .split(inner);

    render_cpu(f, chunks[0], metrics, cpu_history, theme);
    render_memory(f, chunks[1], metrics, mem_history, theme);
    render_load(f, chunks[2], metrics, theme);
    render_disks(f, chunks[3], metrics, theme);
    render_network(f, chunks[4], metrics, net_rx_history, net_tx_history, theme);
    render_processes(f, chunks[5], metrics, sort, process_scroll, theme);
    render_monitor_footer(f, chunks[6], sort, theme);
}

fn render_cpu(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    history: &MetricHistory,
    theme: &Theme,
) {
    let cpu_data = history.as_slice_vec();
    let sparkline_width = (area.width as usize).saturating_sub(16);
    let spark_str = widgets::sparkline_string(&cpu_data, sparkline_width);

    let cpu_bar_width = (area.width as usize).saturating_sub(20);
    let bar = widgets::bar_gauge(metrics.cpu_percent, cpu_bar_width);

    // Per-core summary inline
    let core_summary: String = metrics
        .cpu_per_core
        .iter()
        .enumerate()
        .take(8)
        .map(|(i, &pct)| format!("C{}:{:.0}%", i, pct))
        .collect::<Vec<_>>()
        .join(" ");

    let lines = vec![
        Line::from(vec![
            Span::styled(" CPU  ", Style::default().fg(theme.brand).bold()),
            Span::styled(
                format!("{:5.1}%  ", metrics.cpu_percent),
                Style::default().fg(widgets::pct_color(theme, metrics.cpu_percent)),
            ),
            Span::styled(
                spark_str,
                Style::default().fg(widgets::pct_color(theme, metrics.cpu_percent)),
            ),
        ]),
        Line::from(vec![
            Span::raw("      "),
            Span::styled(
                bar,
                Style::default().fg(widgets::pct_color(theme, metrics.cpu_percent)),
            ),
            Span::raw(format!(" {:.0}%  ", metrics.cpu_percent)),
            Span::styled(core_summary, Style::default().fg(theme.text_muted)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_memory(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    history: &MetricHistory,
    theme: &Theme,
) {
    let total = widgets::format_kb(metrics.mem_total_kb);
    let used = widgets::format_kb(metrics.mem_used_kb);
    let mem_pct = if metrics.mem_total_kb > 0 {
        metrics.mem_used_kb as f64 / metrics.mem_total_kb as f64 * 100.0
    } else {
        0.0
    };

    let mem_data = history.as_slice_vec();
    let sparkline_width = (area.width as usize).saturating_sub(16);
    let spark_str = widgets::sparkline_string(&mem_data, sparkline_width);

    let swap_text = format!(
        "  Swap: {} / {}",
        widgets::format_kb(metrics.mem_swap_used_kb),
        widgets::format_kb(metrics.mem_swap_total_kb)
    );

    let lines = vec![
        Line::from(vec![
            Span::styled(" MEM  ", Style::default().fg(theme.brand).bold()),
            Span::styled(
                format!("{:5.1}%  ", mem_pct),
                Style::default().fg(widgets::pct_color(theme, mem_pct)),
            ),
            Span::styled(
                spark_str,
                Style::default().fg(widgets::pct_color(theme, mem_pct)),
            ),
        ]),
        Line::from(vec![
            Span::raw(format!("      {} / {}", used, total)),
            Span::styled(swap_text, Style::default().fg(theme.text_muted)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_load(f: &mut Frame, area: Rect, metrics: &HostMetrics, theme: &Theme) {
    let uptime = widgets::format_uptime(metrics.uptime_secs);
    let line = Line::from(vec![
        Span::styled(" LOAD ", Style::default().fg(theme.brand).bold()),
        Span::raw(format!(
            "{:.2}  {:.2}  {:.2}",
            metrics.load_1m, metrics.load_5m, metrics.load_15m
        )),
        Span::raw("    "),
        Span::styled("UP ", Style::default().fg(theme.brand).bold()),
        Span::raw(uptime),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
}

fn render_disks(f: &mut Frame, area: Rect, metrics: &HostMetrics, theme: &Theme) {
    let header = Row::new(vec![
        Cell::from(" DISK").style(Style::default().fg(theme.brand).bold()),
        Cell::from("Used").style(Style::default().fg(theme.text_muted)),
        Cell::from("Avail").style(Style::default().fg(theme.text_muted)),
        Cell::from("Use%").style(Style::default().fg(theme.text_muted)),
    ])
    .height(1);

    let max_rows = area.height.saturating_sub(2) as usize; // subtract header + bottom border
    let rows: Vec<Row> = metrics
        .disks
        .iter()
        .take(max_rows.min(15))
        .map(|disk| {
            let avail = disk.total_bytes.saturating_sub(disk.used_bytes);
            let bar = widgets::bar_gauge(disk.use_pct, 10);
            Row::new(vec![
                Cell::from(format!(" {}", disk.mount)),
                Cell::from(widgets::format_bytes(disk.used_bytes)),
                Cell::from(widgets::format_bytes(avail)),
                Cell::from(format!("{} {:.0}%", bar, disk.use_pct))
                    .style(Style::default().fg(widgets::pct_color(theme, disk.use_pct))),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(14),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(16),
    ];

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn render_network(
    f: &mut Frame,
    area: Rect,
    metrics: &HostMetrics,
    rx_history: &MetricHistory,
    tx_history: &MetricHistory,
    theme: &Theme,
) {
    let spark_width = (area.width as usize / 2).saturating_sub(16);
    let rx_data = rx_history.as_slice_vec();
    let tx_data = tx_history.as_slice_vec();
    let rx_spark = widgets::sparkline_string(&rx_data, spark_width);
    let tx_spark = widgets::sparkline_string(&tx_data, spark_width);

    let line = Line::from(vec![
        Span::styled(" NET  ", Style::default().fg(theme.brand).bold()),
        Span::styled("RX ", Style::default().fg(theme.rx_rate)),
        Span::styled(rx_spark, Style::default().fg(theme.rx_rate)),
        Span::raw(format!(
            " {}  ",
            widgets::format_bytes_rate(metrics.net_rx_bps)
        )),
        Span::styled("TX ", Style::default().fg(theme.tx_rate)),
        Span::styled(tx_spark, Style::default().fg(theme.tx_rate)),
        Span::raw(format!(
            " {}",
            widgets::format_bytes_rate(metrics.net_tx_bps)
        )),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
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

    let sort_label = match sort {
        ProcessSort::Cpu => "by CPU",
        ProcessSort::Memory => "by MEM",
    };

    let name_header = format!("Name ({})", sort_label);
    let header = Row::new(vec![
        Cell::from(" PROC").style(Style::default().fg(theme.brand).bold()),
        Cell::from(name_header).style(Style::default().fg(theme.text_muted)),
        Cell::from("CPU%").style(Style::default().fg(theme.text_muted)),
        Cell::from("MEM%").style(Style::default().fg(theme.text_muted)),
        Cell::from("RSS").style(Style::default().fg(theme.text_muted)),
    ])
    .height(1);

    let max_rows = area.height.saturating_sub(1) as usize; // subtract header
    let rows: Vec<Row> = procs
        .iter()
        .skip(scroll)
        .take(max_rows.min(15))
        .map(|p| {
            Row::new(vec![
                Cell::from(format!(" {}", p.pid)),
                Cell::from(p.name.as_str()),
                Cell::from(format!("{:.1}", p.cpu_pct))
                    .style(Style::default().fg(widgets::pct_color(theme, p.cpu_pct))),
                Cell::from(format!("{:.1}", p.mem_pct))
                    .style(Style::default().fg(widgets::pct_color(theme, p.mem_pct))),
                Cell::from(widgets::format_kb(p.mem_rss_kb)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, area);
}

fn render_monitor_footer(f: &mut Frame, area: Rect, sort: &ProcessSort, theme: &Theme) {
    let sort_hint = match sort {
        ProcessSort::Cpu => "s:Sort(→mem)",
        ProcessSort::Memory => "s:Sort(→cpu)",
    };
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" Esc", Style::default().fg(theme.key_hint)),
        Span::raw(":Terminal  "),
        Span::styled(sort_hint, Style::default().fg(theme.key_hint)),
        Span::raw("  "),
        Span::styled("↑↓", Style::default().fg(theme.key_hint)),
        Span::raw(":Scroll  "),
        Span::styled("t", Style::default().fg(theme.key_hint)),
        Span::raw(":Theme"),
    ]));
    f.render_widget(footer, area);
}
