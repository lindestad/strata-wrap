use crate::model::{App, View, number, prefill_rate, text};
use anyhow::Result;
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, Padding, Paragraph, Row, Sparkline, Table,
        TableState,
    },
};
use serde_json::json;

fn style(color: Color) -> Style {
    Style::default().fg(color)
}
fn value(value: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(value.into(), style(color))
}
fn split(
    area: Rect,
    constraints: impl IntoIterator<Item = Constraint>,
    direction: Direction,
) -> Vec<Rect> {
    Layout::default()
        .direction(direction)
        .spacing(if direction == Direction::Horizontal {
            1
        } else {
            0
        })
        .constraints(constraints)
        .split(area)
        .to_vec()
}
fn line(f: &mut Frame, area: Rect, y: u16, text: impl Into<Line<'static>>) {
    if y < area.height {
        f.render_widget(
            Paragraph::new(text.into()),
            Rect::new(area.x, area.y + y, area.width, 1),
        );
    }
}
pub fn commas(n: u64) -> String {
    let chars = n.to_string();
    let mut out = String::new();
    for (i, c) in chars.chars().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 10000 {
        format!("{:.1}k", n as f64 / 1000.)
    } else {
        commas(n)
    }
}
fn gib(n: u64) -> String {
    format!("{:.1}", n as f64 / 1073741824.)
}
/// Money, as the session earned it: two digits at a dollar up, finer while it is cheap.
fn dollars(v: f64) -> String {
    if v >= 100.0 {
        format!("${v:.0}")
    } else if v >= 1.0 {
        format!("${v:.2}")
    } else if v >= 0.01 {
        format!("${v:.3}")
    } else {
        format!("${v:.4}")
    }
}
fn opt_gib(n: Option<u64>) -> String {
    n.map(|n| format!("{} GiB", gib(n)))
        .unwrap_or_else(|| "—".into())
}
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}
pub fn process_height(height: u16) -> u16 {
    let cap = (height / 4).min(12);
    if cap >= 6 { cap } else { 0 }
}

pub fn visible_rows(app: &App, height: u16) -> usize {
    let process = process_height(height).min(app.hardware.processes.len().max(3) as u16 + 3);
    let body = height.saturating_sub(2 + 4 + 3 + resource_height(app, height) + process + 1);
    let reserved = if app.view == View::Requests {
        if body >= 12 { 8 } else { 3 }
    } else {
        2
    };
    body.saturating_sub(reserved).max(1) as usize
}

fn resource_height(app: &App, height: u16) -> u16 {
    if app.view != View::Overview {
        if height >= 36 { 5 } else { 0 }
    } else if height >= 32 {
        7
    } else {
        5
    }
}

struct Ui {
    palette: crate::theme::Palette,
}
impl Ui {
    fn dim(&self, value: impl Into<String>) -> Span<'static> {
        Span::styled(value.into(), style(self.palette.muted))
    }
    fn panel(&self, title: impl Into<String>) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(style(self.palette.border))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.palette.card).fg(self.palette.fg))
            .title(Line::from(format!(" {} ", title.into())).style(style(self.palette.muted)))
    }
    fn inside(&self, f: &mut Frame, area: Rect, title: impl Into<String>) -> Rect {
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    }
    fn gauge(&self, f: &mut Frame, area: Rect, used: u64, total: u64, label: String, color: Color) {
        if area.height == 0 {
            return;
        }
        f.render_widget(
            Gauge::default()
                .gauge_style(style(color).bg(self.palette.track))
                .ratio(if total > 0 {
                    (used as f64 / total as f64).clamp(0., 1.)
                } else {
                    0.
                })
                .label(label)
                .use_unicode(true),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    pub fn render(&self, f: &mut Frame, app: &App) {
        let area = f.area();
        f.render_widget(
            Block::default().style(Style::default().bg(self.palette.bg).fg(self.palette.fg)),
            area,
        );
        if area.width < 50 || area.height < 14 {
            self.small(f, app, area);
            if app.theme_picker.is_some() {
                self.themes(f, app, area);
            }
            if let Some(preview) = &app.reload_preview {
                self.reload(f, preview, area);
            }
            return;
        }
        let process =
            process_height(area.height).min(app.hardware.processes.len().max(3) as u16 + 3);
        let rows = split(
            area,
            [
                Constraint::Length(2),
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Length(resource_height(app, area.height)),
                Constraint::Min(1),
                Constraint::Length(process),
                Constraint::Length(1),
            ],
            Direction::Vertical,
        );
        self.header(f, app, rows[0]);
        self.stats(f, app, rows[1]);
        self.activity(f, app, rows[2]);
        self.resources(f, app, rows[3]);
        match app.view {
            View::Overview => self.overview(f, app, rows[4]),
            View::Requests => self.requests(f, app, rows[4], true),
            View::Logs => self.logs(f, app, rows[4]),
            View::Policy => self.policy(f, app, rows[4]),
        }
        if process > 0 {
            self.processes(f, app, rows[5]);
        }
        if area.width < 90 {
            line(
                f,
                rows[6],
                0,
                Line::from(vec![
                    self.dim(" 1–4 views  hjkl  a ages  t theme  ?"),
                    value(
                        if app.stopping {
                            "  Stopping…"
                        } else {
                            "  q quit"
                        },
                        if app.stopping {
                            self.palette.warning
                        } else {
                            self.palette.accent
                        },
                    ),
                ]),
            );
        } else {
            line(
                f,
                rows[6],
                0,
                Line::from(vec![
                    self.dim(" "),
                    value("1", self.palette.accent),
                    self.dim(" Overview  "),
                    value("2", self.palette.accent),
                    self.dim(" Requests  "),
                    value("3", self.palette.accent),
                    self.dim(" Logs  "),
                    value("4", self.palette.accent),
                    self.dim(" Policy"),
                    self.dim(if area.width >= 94 {
                        "   hjkl  a ages  Space growth  c config  ?"
                    } else {
                        "   hjkl  t theme  ? help"
                    }),
                    value(
                        if app.stopping {
                            "   Stopping…"
                        } else {
                            if app.attached {
                                "   q detach"
                            } else {
                                "   q stop"
                            }
                        },
                        if app.stopping {
                            self.palette.warning
                        } else {
                            self.palette.muted
                        },
                    ),
                ]),
            );
        }
        if app.help {
            self.help(f, app, area);
        }
        if app.theme_picker.is_some() {
            self.themes(f, app, area);
        }
        if let Some(preview) = &app.reload_preview {
            self.reload(f, preview, area);
        } else if let Some(notice) = &app.control_notice {
            line(
                f,
                area,
                area.height.saturating_sub(1),
                Line::from(value(
                    format!(" {notice} · Esc dismiss"),
                    self.palette.warning,
                )),
            );
        }
    }

    fn header(&self, f: &mut Frame, app: &App, area: Rect) {
        let badge = if app.stopping {
            "STOPPING"
        } else if app.stale() {
            "RECONNECTING"
        } else if !app.connected {
            "LOADING"
        } else {
            match text(&app.metrics.data, "/live/state") {
                "generating" => "GENERATING",
                "reading" => "READING",
                "unloaded" => "UNLOADED",
                _ => "READY",
            }
        };
        let color = if app.stopping || app.stale() {
            self.palette.warning
        } else if app.metrics.busy() {
            self.palette.accent
        } else {
            self.palette.success
        };
        let top = split(
            area,
            [
                Constraint::Min(10),
                Constraint::Length(badge.len() as u16 + 4),
            ],
            Direction::Horizontal,
        );
        line(
            f,
            top[0],
            0,
            Line::from(vec![
                value(" STRATA", self.palette.accent).add_modifier(Modifier::BOLD),
                self.dim("  /  "),
                value(app.model.clone(), self.palette.fg),
            ]),
        );
        line(
            f,
            top[1],
            0,
            Line::from(vec![value("● ", color), value(badge, color)]),
        );
        let engine = text(&app.metrics.status, "/engine");
        let mut parts = vec![
            self.dim(format!(
                " {}{}",
                app.address,
                if app.demo {
                    "   PREVIEW"
                } else if app.attached {
                    "   ATTACHED · read-only"
                } else {
                    ""
                }
            )),
            self.dim(format!(
                "   {}",
                if engine.is_empty() {
                    "starting engine"
                } else {
                    engine
                }
            )),
            self.dim(format!("   {}", app.theme.name())),
            self.dim(format!(
                "   session {}",
                clock(app.started.elapsed().as_secs_f64())
            )),
        ];
        if let Some(pricing) = app.pricing {
            // what the session's tokens would cost at the model config's rates
            let totals = app.metrics.totals();
            parts.push(self.dim(format!(
                "   {}{}",
                if totals.partial { "≥" } else { "" },
                dollars(pricing.cost(&totals))
            )));
        }
        line(f, area, 1, Line::from(parts));
    }

    fn stats(&self, f: &mut Frame, app: &App, area: Rect) {
        let total = app.metrics.totals();
        let state = text(&app.metrics.data, "/live/state");
        let (title, rate, subtitle) = match state {
            "reading" => (
                "Prefill",
                app.metrics
                    .data
                    .pointer("/live/prefill_tok_s_mean")
                    .and_then(serde_json::Value::as_f64),
                "live prompt",
            ),
            "generating" => (
                "Decode",
                app.metrics
                    .data
                    .pointer("/live/tok_s")
                    .and_then(serde_json::Value::as_f64),
                "live output",
            ),
            _ => (
                "Decode",
                app.metrics
                    .last()
                    .get("decode_tok_s")
                    .and_then(serde_json::Value::as_f64),
                "last request",
            ),
        };
        let areas = split(area, [Constraint::Percentage(25); 4], Direction::Horizontal);
        let cards = [
            (
                title,
                rate.filter(|r| r.is_finite() && *r >= 0.)
                    .map(|r| format!("{r:.1} tok/s"))
                    .unwrap_or_else(|| "— tok/s".into()),
                subtitle.to_string(),
                self.palette.accent,
            ),
            (
                "Session avg",
                if total.partial {
                    "— tok/s".into()
                } else {
                    format!("{:.1} tok/s", total.tps)
                },
                if number(&app.metrics.data, "/live/parallel") > 0. {
                    "per-request decode mean".into()
                } else {
                    format!("{} decoding", clock(total.decode_seconds))
                },
                self.palette.secondary,
            ),
            (
                "Input tokens",
                format!(
                    "{}{}",
                    if total.partial { "≥" } else { "" },
                    commas(total.input)
                ),
                format!("{} reused", compact(total.reused)),
                self.palette.fg,
            ),
            (
                "Output tokens",
                format!(
                    "{}{}",
                    if total.partial { "≥" } else { "" },
                    commas(total.output)
                ),
                format!("{} requests", total.requests),
                self.palette.success,
            ),
        ];
        for (i, (title, big, sub, color)) in cards.into_iter().enumerate() {
            let inner = self.inside(f, areas[i], title);
            line(
                f,
                inner,
                0,
                Line::from(value(big, color).add_modifier(Modifier::BOLD)),
            );
            line(f, inner, 1, Line::from(self.dim(sub)));
        }
    }

    fn activity(&self, f: &mut Frame, app: &App, area: Rect) {
        let inner = self.inside(f, area, "Now");
        let data = &app.metrics.data;
        let state = text(data, "/live/state");
        let label = if app.stopping {
            "Stopping the server and its engine…".into()
        } else if app.stale() {
            "Monitoring connection interrupted · management waits for fresh data".into()
        } else if !app.connected {
            app.loading_note.clone()
        } else if state == "reading" {
            format!(
                "Reading prompt  {} / {} tokens  ·  {:.1}s",
                commas(number(data, "/live/prompt_read") as u64),
                commas(number(data, "/live/prompt_total") as u64),
                number(data, "/live/elapsed_s")
            )
        } else if state == "generating" {
            format!(
                "Generating  {} tokens  ·  {:.1} tok/s  ·  {:.1}s  ·  {} queued",
                commas(number(data, "/live/generated") as u64),
                number(data, "/live/tok_s"),
                number(data, "/live/elapsed_s"),
                number(data, "/live/queued")
            )
        } else if state == "unloaded" {
            "Model unloaded · waiting for a request".into()
        } else {
            "Ready for the next request".into()
        };
        let (used, total) = if state == "reading" {
            (
                number(data, "/live/prompt_read") as u64,
                number(data, "/live/prompt_total") as u64,
            )
        } else if state == "generating" {
            (
                number(data, "/live/generated") as u64,
                number(data, "/live/max_tokens") as u64,
            )
        } else {
            (0, 1)
        };
        self.gauge(
            f,
            inner,
            used,
            total,
            label,
            if state == "reading" {
                self.palette.secondary
            } else {
                self.palette.accent
            },
        );
    }

    fn resources(&self, f: &mut Frame, app: &App, area: Rect) {
        let cols = split(area, [Constraint::Percentage(50); 2], Direction::Horizontal);
        let hw = &app.hardware;
        let left = self.inside(
            f,
            cols[0],
            if hw.gpu_name.is_empty() {
                "GPU".into()
            } else {
                hw.gpu_name.replace("NVIDIA GeForce ", "")
            },
        );
        let danger = hw.gpu_free < app.min_free_mib.saturating_mul(1048576) && hw.gpu_total > 0;
        self.gauge(
            f,
            left,
            hw.gpu_used,
            hw.gpu_total,
            if hw.gpu_total == 0 {
                "VRAM unavailable".into()
            } else if left.width < 44 {
                format!(
                    "VRAM {}/{}G · {}G free",
                    gib(hw.gpu_used),
                    gib(hw.gpu_total),
                    gib(hw.gpu_free)
                )
            } else {
                format!(
                    "VRAM {} / {} GiB   {} free",
                    gib(hw.gpu_used),
                    gib(hw.gpu_total),
                    gib(hw.gpu_free)
                )
            },
            if danger {
                self.palette.error
            } else {
                self.palette.accent
            },
        );
        line(
            f,
            left,
            1,
            Line::from(vec![
                self.dim("Load "),
                value(
                    hw.gpu_util
                        .map(|v| format!("{v}%"))
                        .unwrap_or_else(|| "—".into()),
                    self.palette.fg,
                ),
                self.dim("   Temp "),
                value(
                    hw.gpu_temp
                        .map(|v| format!("{v}°C"))
                        .unwrap_or_else(|| "—".into()),
                    if hw.gpu_temp.unwrap_or(0) >= 80 {
                        self.palette.warning
                    } else {
                        self.palette.fg
                    },
                ),
                self.dim("   Power "),
                value(
                    hw.gpu_power
                        .map(|v| format!("{v:.0}W"))
                        .unwrap_or_else(|| "—".into()),
                    self.palette.fg,
                ),
            ]),
        );
        line(
            f,
            left,
            2,
            Line::from(vec![
                self.dim(format!(
                    "PCIe {}×{}",
                    hw.pcie_gen.unwrap_or(0),
                    hw.pcie_width.unwrap_or(0)
                )),
                value(
                    format!("  {:.0} MiB/s → GPU", hw.pcie_rx.unwrap_or(0.)),
                    self.palette.fg,
                ),
            ]),
        );
        if left.height >= 5 {
            let points = app.gpu_load.iter().copied().collect::<Vec<_>>();
            f.render_widget(
                Sparkline::default()
                    .data(&points)
                    .max(100)
                    .style(style(self.palette.accent)),
                Rect::new(left.x, left.y + 3, left.width, 1),
            );
            let owned = hw
                .processes
                .iter()
                .filter(|p| p.owned)
                .filter_map(|p| p.vram)
                .sum::<u64>();
            line(
                f,
                left,
                4,
                Line::from(self.dim(if let Some(error) = &hw.gpu_error {
                    error.clone()
                } else {
                    format!(
                        "Strata {}G · other {}G · {} {}G",
                        gib(owned),
                        gib(hw.gpu_allocated().saturating_sub(owned)),
                        if left.width < 44 { "res" } else { "reserved" },
                        gib(hw.gpu_reserved)
                    )
                })),
            );
        }
        let right = self.inside(f, cols[1], "System memory & CPU");
        self.gauge(
            f,
            right,
            hw.ram_used,
            hw.ram_total,
            if right.width < 44 {
                format!(
                    "RAM {}/{}G · {}G avail",
                    gib(hw.ram_used),
                    gib(hw.ram_total),
                    gib(hw.ram_available)
                )
            } else {
                format!(
                    "RAM {} / {} GiB   {} available",
                    gib(hw.ram_used),
                    gib(hw.ram_total),
                    gib(hw.ram_available)
                )
            },
            if hw.ram_available < hw.ram_total / 12 {
                self.palette.warning
            } else {
                self.palette.secondary
            },
        );
        line(
            f,
            right,
            1,
            Line::from(vec![
                self.dim("Model PSS "),
                value(opt_gib(hw.model_pss), self.palette.fg),
                self.dim("   locked "),
                value(opt_gib(hw.model_locked), self.palette.fg),
            ]),
        );
        line(
            f,
            right,
            2,
            Line::from(vec![
                self.dim(format!("CPU {:.0}%", hw.cpu)),
                value(
                    format!(
                        "  read {:.0} · write {:.1} MiB/s",
                        hw.disk_read, hw.disk_write
                    ),
                    self.palette.fg,
                ),
            ]),
        );
        if right.height >= 5 {
            line(
                f,
                right,
                3,
                Line::from(self.dim(format!(
                    "Anon {}G · file {}G · shared {}G",
                    hw.model_anon.map(gib).unwrap_or_else(|| "—".into()),
                    hw.model_file.map(gib).unwrap_or_else(|| "—".into()),
                    hw.model_shared.map(gib).unwrap_or_else(|| "—".into())
                ))),
            );
            self.gauge(
                f,
                Rect::new(right.x, right.y + 4, right.width, 1),
                hw.swap_used,
                hw.swap_total,
                format!("Swap {} / {} GiB", gib(hw.swap_used), gib(hw.swap_total)),
                self.palette.secondary,
            );
        }
    }

    fn overview(&self, f: &mut Frame, app: &App, area: Rect) {
        if area.height < 5 {
            let inner = self.inside(f, area, "Context & expert cache");
            line(
                f,
                inner,
                0,
                Line::from(self.dim(format!(
                    "Context {}/{} · cache {:.1}/{:.1} GiB · {}",
                    compact(number(&app.metrics.data, "/live/prompt_tokens") as u64),
                    compact(number(&app.metrics.data, "/engine/max_context") as u64),
                    app.metrics.cache() as f64 / 1024.,
                    app.metrics.full_cache() as f64 / 1024.,
                    app.policy
                ))),
            );
            return;
        }
        if area.width < 70 {
            self.runtime(f, app, area);
            return;
        }
        let cols = split(
            area,
            [Constraint::Percentage(64), Constraint::Percentage(36)],
            Direction::Horizontal,
        );
        if area.height >= 12 {
            let rows = split(
                cols[0],
                [Constraint::Length(5), Constraint::Min(3)],
                Direction::Vertical,
            );
            let inner = self.inside(f, rows[0], "Decode rhythm · last minute");
            let points = app
                .tps
                .iter()
                .rev()
                .take(inner.width as usize)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>();
            f.render_widget(
                Sparkline::default()
                    .data(&points)
                    .max(points.iter().copied().max().unwrap_or(500).max(100))
                    .style(style(self.palette.accent)),
                inner,
            );
            self.requests(f, app, rows[1], false);
        } else {
            self.requests(f, app, cols[0], false);
        }
        self.runtime(f, app, cols[1]);
    }

    fn runtime(&self, f: &mut Frame, app: &App, area: Rect) {
        let inner = self.inside(f, area, "Context & elastic cache");
        let m = &app.metrics;
        let data = &m.data;
        let context = number(data, "/engine/max_context") as u64;
        let active = m.busy();
        let used = if active {
            number(data, "/live/prompt_tokens") + number(data, "/live/generated")
        } else {
            number(m.last(), "/prompt_tokens") + number(m.last(), "/output_tokens")
        };
        self.gauge(
            f,
            Rect::new(
                inner.x,
                inner.y.saturating_add(2),
                inner.width,
                inner.height.saturating_sub(2),
            ),
            used as u64,
            context,
            format!("Context {} / {}", compact(used as u64), compact(context)),
            self.palette.accent,
        );
        line(
            f,
            inner,
            3,
            Line::from(self.dim(format!(
                "KV {} · {} resident",
                text(data, "/engine/kv"),
                compact(number(data, "/engine/kv_resident") as u64)
            ))),
        );
        if inner.height > 2 {
            self.gauge(
                f,
                Rect::new(inner.x, inner.y + 1, inner.width, 1),
                m.cache(),
                m.full_cache(),
                format!(
                    "Experts {:.1} / {:.1} GiB",
                    m.cache() as f64 / 1024.,
                    m.full_cache() as f64 / 1024.
                ),
                self.palette.secondary,
            );
        }
        line(
            f,
            inner,
            4,
            Line::from(self.dim(format!(
                "{} expert slots · {} MiB steps",
                commas(number(data, "/engine/expert_slots") as u64),
                app.step_mib
            ))),
        );
        line(
            f,
            inner,
            0,
            Line::from(value(
                app.supervisor.as_ref().map_or_else(
                    || app.policy.clone(),
                    |s| {
                        format!(
                            "{} · {}",
                            match s.effective.growth_mode {
                                crate::policy::GrowthMode::Idle => "idle",
                                crate::policy::GrowthMode::Capacity => "capacity",
                            },
                            app.policy
                        )
                    },
                ),
                if app.resize_pending {
                    self.palette.warning
                } else {
                    self.palette.accent
                },
            )),
        );
        line(
            f,
            inner,
            5,
            Line::from(self.dim(format!(
                "Reserve ≥{:.1} GiB · grow <{:.1} GiB",
                app.min_free_mib as f64 / 1024.0,
                app.grow_below_mib as f64 / 1024.0
            ))),
        );
        let last = m.last();
        let offered = number(last, "/drafts_offered");
        line(
            f,
            inner,
            7,
            Line::from(vec![
                self.dim("MTP acceptance "),
                value(
                    if offered > 0. {
                        format!("{:.1}%", number(last, "/drafts_accepted") / offered * 100.)
                    } else {
                        "—".into()
                    },
                    self.palette.success,
                ),
            ]),
        );
        line(
            f,
            inner,
            8,
            Line::from(vec![
                self.dim("Prefill "),
                value(
                    (if active {
                        data.pointer("/live/prefill_tok_s_mean")
                            .and_then(serde_json::Value::as_f64)
                    } else {
                        prefill_rate(last)
                    })
                    .map(|r| format!("{r:.0} tok/s"))
                    .unwrap_or_else(|| "— tok/s".into()),
                    self.palette.fg,
                ),
            ]),
        );
        let cache = &data["conversation_cache"];
        line(
            f,
            inner,
            10,
            Line::from(self.dim(format!(
                "Conversation cache · {} / {} parked",
                number(cache, "/parked"),
                number(cache, "/slots")
            ))),
        );
        line(
            f,
            inner,
            11,
            Line::from(vec![
                value(
                    format!(
                        "{} / {:.1} GiB",
                        gib(number(cache, "/bytes") as u64),
                        number(cache, "/budget_mib") / 1024.
                    ),
                    self.palette.fg,
                ),
                self.dim(format!(" · {} restores", number(cache, "/restores"))),
            ]),
        );
        line(
            f,
            inner,
            12,
            Line::from(self.dim(format!(
                "{} reused · {} evictions",
                compact(m.totals().reused),
                number(cache, "/evictions")
            ))),
        );
        line(
            f,
            inner,
            14,
            Line::from(self.dim(format!(
                "Model group {} charged",
                opt_gib(app.hardware.model_cgroup)
            ))),
        );
        line(
            f,
            inner,
            15,
            Line::from(self.dim(format!("{} CPU threads", app.hardware.threads))),
        );
    }

    fn policy(&self, f: &mut Frame, app: &App, area: Rect) {
        let rows = split(
            area,
            [
                Constraint::Length(if area.height >= 14 { 6 } else { 2 }),
                Constraint::Min(1),
            ],
            Direction::Vertical,
        );
        if let Some(state) = &app.supervisor {
            let graphs = split(
                rows[0],
                [Constraint::Percentage(50); 2],
                Direction::Horizontal,
            );
            for (i, title) in [
                "Free VRAM · MiB · recent samples",
                "Expert cache · MiB · recent samples",
            ]
            .iter()
            .enumerate()
            {
                let inner = self.inside(f, graphs[i], title.to_string());
                let data = state
                    .memory_history
                    .iter()
                    .rev()
                    .take(inner.width as usize)
                    .map(|(free, cache)| if i == 0 { *free } else { *cache })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>();
                f.render_widget(
                    Sparkline::default()
                        .data(&data)
                        .max(data.iter().copied().max().unwrap_or(1).max(1))
                        .style(style(if i == 0 {
                            self.palette.accent
                        } else {
                            self.palette.secondary
                        })),
                    inner,
                );
            }
            let records = state
                .resize_history
                .iter()
                .rev()
                .map(|r| {
                    Row::new(vec![
                        format!("{}m", ((app.age_now - r.at).max(0.) / 60.) as u64),
                        r.direction.clone(),
                        format!(
                            "{} → {}",
                            r.before_mib,
                            r.after_mib.map_or_else(|| "…".into(), |n| n.to_string())
                        ),
                        r.free_mib.map_or_else(|| "—".into(), |n| n.to_string()),
                        r.target_free_mib.to_string(),
                        format!("{:.1}s", r.wait_seconds),
                        r.result.clone(),
                    ])
                })
                .collect::<Vec<_>>();
            let table = Table::new(
                records,
                [
                    Constraint::Length(5),
                    Constraint::Length(6),
                    Constraint::Length(16),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Min(10),
                ],
            )
            .header(
                Row::new([
                    "Age",
                    "Asked",
                    "Cache MiB",
                    "Free",
                    "Target",
                    "Wait",
                    "Result",
                ])
                .style(style(self.palette.muted)),
            )
            .block(self.panel(format!(
                "{} · {:?} · Space growth · r reconcile · c config",
                state.policy, state.effective.growth_mode
            )))
            .row_highlight_style(Style::default().bg(self.palette.track));
            let mut selected = TableState::default().with_selected(
                (!state.resize_history.is_empty())
                    .then_some(app.scroll.min(state.resize_history.len().saturating_sub(1))),
            );
            f.render_stateful_widget(table, rows[1], &mut selected);
            if state.resize_history.is_empty() {
                let inner = rows[1].inner(ratatui::layout::Margin::new(2, 2));
                line(
                    f,
                    inner,
                    0,
                    Line::from(self.dim("No resize requests this session")),
                );
            }
        } else {
            let inner = self.inside(f, area, "VRAM policy");
            line(
                f,
                inner,
                0,
                Line::from(value(app.policy.clone(), self.palette.accent)),
            );
            line(
                f,
                inner,
                2,
                Line::from(
                    self.dim(
                        "Worker history unavailable · restart an older wrapper when convenient",
                    ),
                ),
            );
        }
    }

    fn reload(&self, f: &mut Frame, preview: &serde_json::Value, area: Rect) {
        let width = area.width.saturating_sub(2).min(90);
        let height = area.height.saturating_sub(2).min(20);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        f.render_widget(Clear, rect);
        let inner = self.inside(f, rect, "Reload VRAM policy · Enter apply · Esc cancel");
        let current = &preview["current"];
        let proposed = &preview["proposed"];
        let mut lines = vec![Line::from(
            self.dim("Setting                     Running → Config"),
        )];
        for key in [
            "enabled",
            "growth_mode",
            "min_free_mib",
            "grow_below_mib",
            "grow_delay_seconds",
            "poll_seconds",
            "step_mib",
            "accounting_margin_mib",
            "no_op_retry_seconds",
        ] {
            let changed = current[key] != proposed[key];
            lines.push(Line::from(value(
                format!("{key:<27} {} → {}", current[key], proposed[key]),
                if changed {
                    self.palette.accent
                } else {
                    self.palette.muted
                },
            )));
        }
        lines.push(Line::from(self.dim(
            "VRAM settings only. Growth timer restarts. GPU/model require restart.",
        )));
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn requests(&self, f: &mut Frame, app: &App, area: Rect, detail: bool) {
        let rows = if detail && area.height >= 12 {
            split(
                area,
                [Constraint::Min(6), Constraint::Length(5)],
                Direction::Vertical,
            )
        } else {
            vec![area]
        };
        let requests = app.metrics.data["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let wide = area.width >= 86;
        let header = if wide {
            vec![
                "Age",
                "Input",
                "Reused",
                "Output",
                "Prefill/s",
                "Decode/s",
                "Hit",
                "Wall",
                "Result",
            ]
        } else {
            vec!["Age", "Input", "Output", "tok/s", "Result"]
        };
        let latest = app.age_now;
        let records = requests
            .iter()
            .map(|r| {
                let status = text(r, "/finish");
                let color = if matches!(status, "error" | "cancel" | "disconnect") {
                    self.palette.warning
                } else {
                    self.palette.success
                };
                let elapsed = (latest - number(r, "/time")).max(0.);
                let age = if app.age_seconds {
                    clock(elapsed)
                } else {
                    format!("{}m", elapsed as u64 / 60)
                };
                let mut cells = vec![
                    Cell::from(age).style(style(self.palette.muted)),
                    Cell::from(compact(number(r, "/prompt_tokens") as u64)),
                ];
                if wide {
                    cells.push(Cell::from(compact(number(r, "/reused") as u64)));
                }
                cells.push(Cell::from(compact(number(r, "/output_tokens") as u64)));
                if wide {
                    cells.push(Cell::from(
                        prefill_rate(r)
                            .map(|rate| format!("{rate:.0}"))
                            .unwrap_or_else(|| "—".into()),
                    ));
                }
                cells.push(
                    Cell::from(format!("{:.1}", number(r, "/decode_tok_s")))
                        .style(style(self.palette.accent)),
                );
                if wide {
                    cells.push(Cell::from(format!("{:.0}%", number(r, "/hit_rate") * 100.)));
                    cells.push(Cell::from(format!("{:.1}s", number(r, "/duration_s"))));
                }
                cells.push(Cell::from(status.to_string()).style(style(color)));
                Row::new(cells)
            })
            .collect::<Vec<_>>();
        let widths = if wide {
            vec![
                Constraint::Length(8),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(10),
            ]
        } else {
            vec![
                Constraint::Length(8),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Length(8),
                Constraint::Length(10),
            ]
        };
        let table = Table::new(records, widths)
            .header(Row::new(header).style(style(self.palette.muted)).height(1))
            .block(self.panel(if detail {
                "Request history · j/k select · a ages"
            } else {
                "Recent requests · Enter details · a ages"
            }))
            .column_spacing(1)
            .row_highlight_style(Style::default().bg(self.palette.track))
            .highlight_symbol(if detail { "› " } else { "" });
        let mut state = TableState::default();
        if !requests.is_empty() {
            state.select(Some(app.scroll.min(requests.len() - 1)));
        }
        f.render_stateful_widget(table, rows[0], &mut state);
        if requests.is_empty() && rows[0].height >= 4 {
            line(
                f,
                rows[0].inner(ratatui::layout::Margin::new(2, 2)),
                0,
                Line::from(self.dim("No requests yet")),
            );
        }
        if rows.len() > 1 {
            let inner = self.inside(f, rows[1], "Selected request");
            if let Some(r) = requests.get(app.scroll.min(requests.len().saturating_sub(1))) {
                line(
                    f,
                    inner,
                    0,
                    Line::from(vec![
                        self.dim("Input "),
                        value(commas(number(r, "/prompt_tokens") as u64), self.palette.fg),
                        self.dim("   cached "),
                        value(commas(number(r, "/reused") as u64), self.palette.secondary),
                        self.dim("   output "),
                        value(
                            commas(number(r, "/output_tokens") as u64),
                            self.palette.success,
                        ),
                    ]),
                );
                line(
                    f,
                    inner,
                    1,
                    Line::from(self.dim(format!(
                        "Prefill {:.2}s · decode {:.2}s · total {:.1}s · {:.1} tok/s",
                        number(r, "/prompt_ms") / 1000.,
                        number(r, "/decode_ms") / 1000.,
                        number(r, "/duration_s"),
                        number(r, "/decode_tok_s")
                    ))),
                );
                line(
                    f,
                    inner,
                    2,
                    Line::from(self.dim(format!(
                        "MTP {:.0}/{:.0} accepted · expert hit {:.1}% · PCIe share {:.1}%",
                        number(r, "/drafts_accepted"),
                        number(r, "/drafts_offered"),
                        number(r, "/hit_rate") * 100.,
                        number(r, "/pcie_share") * 100.
                    ))),
                );
            }
        }
    }

    fn processes(&self, f: &mut Frame, app: &App, area: Rect) {
        let rows = &app.hardware.processes;
        let capacity = area.height.saturating_sub(3) as usize;
        let title = format!(
            "GPU processes · {}{}",
            rows.len(),
            if rows.len() > capacity {
                format!(" · showing {capacity}")
            } else {
                String::new()
            }
        );
        let data = rows
            .iter()
            .skip(app.process_offset)
            .take(capacity)
            .map(|p| {
                let color = if p.owned {
                    self.palette.accent
                } else {
                    self.palette.fg
                };
                let memory = p
                    .vram
                    .map(|v| format!("{} MiB", commas(v >> 20)))
                    .unwrap_or_else(|| "unavailable".into());
                Row::new(vec![
                    Cell::from(p.pid.to_string()).style(style(self.palette.muted)),
                    Cell::from(format!("{}{}", if p.owned { "● " } else { "  " }, p.name))
                        .style(style(color)),
                    Cell::from(p.kind.clone()).style(style(self.palette.muted)),
                    Cell::from(memory).style(style(color)),
                    Cell::from(
                        p.vram
                            .map(|v| {
                                format!(
                                    "{:.1}%",
                                    v as f64 / app.hardware.gpu_total.max(1) as f64 * 100.
                                )
                            })
                            .unwrap_or_else(|| "—".into()),
                    )
                    .style(style(self.palette.muted)),
                ])
            })
            .collect::<Vec<_>>();
        f.render_widget(
            Table::new(
                data,
                [
                    Constraint::Length(8),
                    Constraint::Fill(1),
                    Constraint::Length(5),
                    Constraint::Length(13),
                    Constraint::Length(7),
                ],
            )
            .header(
                Row::new(["PID", "Process", "Type", "VRAM", "Share"])
                    .style(style(self.palette.muted)),
            )
            .column_spacing(2)
            .block(
                self.panel(format!(
                    "{title}{}",
                    if app.process_focus {
                        " · j/k scroll · Esc back"
                    } else {
                        " · p focus"
                    }
                ))
                .border_style(style(if app.process_focus {
                    self.palette.accent
                } else {
                    self.palette.border
                })),
            ),
            area,
        );
        if let Some(error) = &app.hardware.process_error {
            line(
                f,
                area.inner(ratatui::layout::Margin::new(2, 2)),
                0,
                Line::from(value(error.clone(), self.palette.warning)),
            );
        }
    }

    fn logs(&self, f: &mut Frame, app: &App, area: Rect) {
        let inner = self.inside(
            f,
            area,
            if app.scroll == 0 {
                "Event log · following"
            } else {
                "Event log · scrolled · End to follow"
            },
        );
        let take = inner.height as usize;
        let end = app
            .logs
            .len()
            .saturating_sub(app.scroll.min(app.logs.len()));
        let start = end.saturating_sub(take);
        let lines = app
            .logs
            .range(start..end)
            .map(|s| {
                Line::from(Span::styled(
                    s.clone(),
                    style(
                        if s.to_lowercase().contains("error") || s.to_lowercase().contains("failed")
                        {
                            self.palette.error
                        } else if s.contains("VRAM") {
                            self.palette.accent
                        } else {
                            self.palette.muted
                        },
                    ),
                ))
            })
            .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn small(&self, f: &mut Frame, app: &App, area: Rect) {
        let t = app.metrics.totals();
        let lines = vec![
            Line::from(value(" STRATA", self.palette.accent)),
            Line::from(app.model.clone()),
            Line::from(value(app.policy.clone(), self.palette.accent)),
            Line::from(format!(
                "Session {:.1} tok/s · {} output",
                t.tps,
                compact(t.output)
            )),
            Line::from(format!(
                "{} input · RAM {} GiB",
                compact(t.input),
                gib(app.hardware.ram_used)
            )),
            Line::from(self.dim("Resize to 50×14 for the dashboard")),
            Line::from(self.dim(if app.attached {
                "t theme · q detach · ? help"
            } else {
                "t theme · q stop · ? help"
            })),
        ];
        f.render_widget(Paragraph::new(lines), area);
    }

    fn themes(&self, f: &mut Frame, app: &App, area: Rect) {
        let width = area.width.saturating_sub(2).min(68);
        let height = area.height.saturating_sub(2).min(16);
        if width < 18 || height < 5 {
            return;
        }
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        f.render_widget(Clear, rect);
        let block = self
            .panel("Themes · live preview")
            .border_style(style(self.palette.accent));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let rows = crate::theme::Theme::ALL.into_iter().map(|theme| {
            Row::new(vec![
                Cell::from(theme.name()),
                Cell::from(theme.description()),
            ])
        });
        let table_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(3),
        );
        let mut state = TableState::default().with_selected(
            crate::theme::Theme::ALL
                .iter()
                .position(|t| *t == app.theme),
        );
        f.render_stateful_widget(
            Table::new(rows, [Constraint::Length(17), Constraint::Min(0)])
                .highlight_symbol("› ")
                .row_highlight_style(
                    Style::default()
                        .fg(self.palette.accent)
                        .bg(self.palette.track)
                        .bold(),
                ),
            table_area,
            &mut state,
        );
        line(
            f,
            inner,
            inner.height.saturating_sub(2),
            Line::from(self.dim(if app.demo {
                "hjkl preview · Enter apply · Esc cancel"
            } else {
                "hjkl preview · Enter save · Esc cancel"
            })),
        );
        line(
            f,
            inner,
            inner.height.saturating_sub(1),
            Line::from(value(
                app.theme_notice.clone().unwrap_or_else(|| {
                    if app.demo {
                        "Preview only · no server or preference changes".into()
                    } else {
                        "Saved choices apply to future launches".into()
                    }
                }),
                if app.theme_notice.is_some() {
                    self.palette.warning
                } else {
                    self.palette.muted
                },
            )),
        );
    }

    fn help(&self, f: &mut Frame, app: &App, area: Rect) {
        let width = area.width.saturating_sub(4).min(80);
        let height = area.height.saturating_sub(2).min(23);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        f.render_widget(Clear, rect);
        let inner = self.inside(f, rect, "Strata · keyboard & metrics");
        let lines = [
            "1 / 2 / 3 / 4 / Tab   Overview / requests / logs / policy",
            "Space / r / c      Pause growth / reconcile / preview VRAM reload",
            "h / l / ← →        Previous / next view",
            "j / k / ↓ ↑        Select requests or scroll logs",
            "a                 Toggle request ages: minutes / seconds",
            "Ctrl+D / Ctrl+U    Half-page down / up",
            "Ctrl+F / Ctrl+B    Page down / up (also PageDown / PageUp)",
            "gg / G             First / last request; oldest / newest log",
            "Enter              Open the selected request's details",
            "p / Esc            Focus GPU processes / return; j/k scroll",
            "b                   Open Strata's web dashboard",
            "t                   Themes · hjkl preview · Enter apply · Esc cancel",
            "? / Esc             Close this help",
            if app.attached {
                "q / Ctrl+C          Detach; keep the server running"
            } else if app.demo {
                "q / Ctrl+C          Close this preview"
            } else {
                "q / Ctrl+C          Stop Strata and exit (again to force)"
            },
            "",
            "Session avg = output tokens / active decode seconds.",
            "Idle and prompt processing do not dilute it. Live output is included.",
            "Input counts include cached tokens; reused tokens are shown separately.",
            "Model PSS shares shared pages proportionally across processes.",
            "Locked memory is part of model memory, not an extra allocation.",
            "GPU processes include graphics and compute, deduplicated by PID.",
            "Driver-reserved memory counts as used VRAM.",
            "VRAM resizing waits behind inference; the reserve is best effort.",
        ];
        f.render_widget(
            Paragraph::new(Text::from(
                lines
                    .into_iter()
                    .map(|l| Line::from(l.to_string()))
                    .collect::<Vec<_>>(),
            ))
            .style(style(self.palette.fg)),
            inner,
        );
    }
}

pub fn render(f: &mut Frame, app: &App) {
    Ui {
        palette: app.theme.palette(),
    }
    .render(f, app);
}

pub fn snapshot(app: &App, width: u16, height: u16, cells: bool) -> Result<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|f| render(f, app))?;
    let buffer = terminal.backend().buffer();
    if cells {
        let mut output = vec![];
        for y in 0..height {
            for x in 0..width {
                let c = &buffer[(x, y)];
                output.push(json!({"x":x,"y":y,"text":c.symbol(),"fg":rgb(c.fg,app.theme.palette().fg),"bg":rgb(c.bg,app.theme.palette().bg),"bold":c.modifier.contains(Modifier::BOLD)}));
            }
        }
        return Ok(serde_json::to_string(
            &json!({"width":width,"height":height,"cells":output}),
        )?);
    }
    let mut output = String::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            row.push_str(buffer[(x, y)].symbol());
        }
        output.push_str(row.trim_end());
        output.push('\n');
    }
    Ok(output)
}
fn rgb(color: Color, fallback: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => match fallback {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => "#ffffff".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dollars_rounds_coarser_as_the_session_earns_more() {
        assert_eq!(dollars(0.00395), "$0.0040");
        assert_eq!(dollars(0.0161), "$0.016");
        assert_eq!(dollars(12.342), "$12.34");
        assert_eq!(dollars(123.4), "$123");
    }

    #[test]
    fn the_header_carries_the_session_cost_only_when_rates_are_known() {
        let text = snapshot(&crate::model::demo(), 120, 40, false).unwrap();
        assert!(
            text.contains("session ") && text.contains("   $0."),
            "{text}"
        );
        let mut app = crate::model::App::new("m".into(), 8080);
        app.connected = true;
        let text = snapshot(&app, 120, 40, false).unwrap();
        assert!(!text.contains('$'), "{text}");
    }

    #[test]
    fn theme_picker_keeps_last_palette_visible_in_a_short_terminal() {
        let mut app = crate::model::demo();
        app.theme = crate::theme::Theme::Everforest;
        app.theme_picker = Some(crate::theme::Theme::Strata);
        let text = snapshot(&app, 80, 14, false).unwrap();
        assert!(text.contains("› everforest"));
        assert!(text.contains("Enter apply"));
    }
    #[test]
    fn live_card_switches_prefill_decode_and_last_request_without_stale_rates() {
        let mut app = crate::model::demo();
        app.metrics.data["live"]["state"] = json!("reading");
        app.metrics.data["live"]["prefill_tok_s_mean"] = json!(1234.5);
        let top = |app: &App| {
            snapshot(app, 100, 32, false)
                .unwrap()
                .lines()
                .take(6)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let prefill = top(&app);
        assert!(prefill.contains("Prefill") && prefill.contains("1234.5 tok/s"));
        assert!(prefill.contains("live prompt") && !prefill.contains("44.8 tok/s"));
        app.metrics.data["live"]["prefill_tok_s_mean"] = serde_json::Value::Null;
        assert!(top(&app).contains("— tok/s"));
        app.metrics.data["live"]["state"] = json!("generating");
        let decode = top(&app);
        assert!(decode.contains("Decode") && decode.contains("44.8 tok/s"));
        assert!(!decode.contains("Prefill"));
        app.metrics.data["live"]["state"] = json!("idle");
        let idle = top(&app);
        assert!(idle.contains("42.7 tok/s") && idle.contains("last request"));
    }

    #[test]
    fn vram_label_and_breakdown_include_driver_reservation() {
        let mut app = crate::model::demo();
        app.hardware.gpu_total = 17_094_934_528;
        app.hardware.gpu_free = 1_725_890_560;
        app.hardware.gpu_used = app.hardware.gpu_total - app.hardware.gpu_free;
        app.hardware.gpu_reserved = 411_369_472;
        for width in [100, 140] {
            let text = snapshot(&app, width, 44, false).unwrap();
            assert!(text.contains("VRAM 14.3 / 15.9 GiB   1.6 free"));
            assert!(text.contains("reserved 0.4G"));
            assert!(text.contains("other 2.9G"));
        }
        let text = snapshot(&app, 80, 32, false).unwrap();
        assert!(text.contains("VRAM 14.3/15.9G · 1.6G free"));
    }
    #[test]
    fn monochrome_has_no_colored_cells_in_any_view_or_overlay() {
        let mut app = crate::model::demo();
        app.theme = crate::theme::Theme::Monochrome;
        for view in [View::Overview, View::Requests, View::Logs, View::Policy] {
            app.view = view;
            for overlay in 0..3 {
                app.help = overlay == 1;
                app.theme_picker = (overlay == 2).then_some(app.theme);
                let mut terminal = Terminal::new(TestBackend::new(140, 48)).unwrap();
                terminal.draw(|f| render(f, &app)).unwrap();
                for cell in terminal.backend().buffer().content() {
                    for color in [cell.fg, cell.bg] {
                        if let Color::Rgb(r, g, b) = color {
                            assert_eq!((r, r), (g, b));
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn process_panel_never_exceeds_quarter_and_requires_three_rows() {
        for h in 1..250 {
            let p = process_height(h);
            assert!(p <= h / 4);
            assert!(p == 0 || p >= 6);
        }
    }
    #[test]
    fn every_terminal_size_renders_without_panicking() {
        let mut app = crate::model::demo();
        for theme in crate::theme::Theme::ALL {
            app.theme = theme;
            for view in [View::Overview, View::Requests, View::Logs, View::Policy] {
                app.view = view;
                for w in [0, 20, 49, 50, 60, 80, 100, 140] {
                    for h in [0, 5, 13, 14, 20, 23, 24, 32, 45, 60] {
                        snapshot(&app, w, h, false).unwrap();
                        app.theme_picker = Some(theme);
                        snapshot(&app, w, h, false).unwrap();
                        app.theme_picker = None;
                        app.help = true;
                        snapshot(&app, w, h, false).unwrap();
                        app.help = false;
                    }
                }
            }
        }
    }
    #[test]
    fn compact_layout_hides_processes() {
        let app = crate::model::demo();
        assert!(
            !snapshot(&app, 100, 23, false)
                .unwrap()
                .contains("GPU processes")
        );
        assert!(
            snapshot(&app, 100, 32, false)
                .unwrap()
                .contains("GPU processes")
        );
    }

    #[test]
    fn small_request_and_log_views_keep_room_for_content() {
        let mut app = crate::model::demo();
        app.view = View::Requests;
        assert!(snapshot(&app, 80, 24, false).unwrap().contains("1,128"));
        app.view = View::Logs;
        assert!(
            snapshot(&app, 80, 24, false)
                .unwrap()
                .contains("Request started")
        );
    }
    #[test]
    fn dashboard_includes_requested_session_and_memory_values() {
        let text = snapshot(&crate::model::demo(), 140, 48, false).unwrap();
        for label in [
            "Session avg",
            "Input tokens",
            "Output tokens",
            "Model PSS",
            "locked",
            "GPU processes",
            "RuneLite",
            "Context",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
    }
    #[test]
    fn policy_remains_visible_in_small_layout_and_history_fills_tall_panels() {
        let mut app = crate::model::demo();
        app.policy = "Growing in 42s · spare VRAM".into();
        assert!(
            snapshot(&app, 60, 20, false)
                .unwrap()
                .contains("Growing in 42s")
        );
        let row = app.metrics.data["requests"][0].clone();
        app.metrics.data["requests"] = json!((0..50).map(|_| row.clone()).collect::<Vec<_>>());
        app.view = View::Requests;
        let text = snapshot(&app, 100, 55, false).unwrap();
        assert!(text.matches("42.7").count() > 12);
        app.view = View::Policy;
        let text = snapshot(&app, 120, 40, false).unwrap();
        assert!(
            text.contains("Free VRAM") && text.contains("7168 → 6656") && text.contains("applied")
        );
        app.view = View::Requests;
        app.age_now = crate::control::epoch();
        let minutes = snapshot(&app, 100, 35, false).unwrap();
        assert!(!minutes.contains("0m 32s"));
        app.age_seconds = true;
        let seconds = snapshot(&app, 100, 35, false).unwrap();
        assert!(seconds.contains("0m 32s"));
    }
}
