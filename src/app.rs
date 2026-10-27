use crate::{
    Args,
    model::{self, App, Event, View, number, text},
    runtime, theme, ui,
};
use anyhow::Result;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{
    io::{BufRead, BufReader, IsTerminal},
    process::{Child, Command, Stdio},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

struct TerminalReset(bool);
impl Drop for TerminalReset {
    fn drop(&mut self) {
        if self.0 {
            ratatui::restore();
        }
    }
}

struct Runner<'a> {
    child: Child,
    args: &'a Args,
}
impl Drop for Runner<'_> {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            runtime::stop_unit(self.args, false);
            let until = Instant::now() + Duration::from_secs(22);
            while Instant::now() < until {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn launch(args: &Args) -> Result<i32> {
    let config = runtime::read_config(args)?;
    let port = args
        .port
        .unwrap_or_else(|| config["port"].as_u64().unwrap_or(8080) as u16);
    let mut app = App::new(
        config["model_name"].as_str().unwrap_or("Strata").into(),
        port,
    );
    app.pricing = model::pricing(&config);
    app.min_free_mib = args.min_free_mib;
    app.grow_below_mib = args.grow_below_mib;
    app.step_mib = args.step_mib;
    app.age_seconds = args.age_seconds;
    configure_theme(&mut app, args);
    let signal = runtime::signals()?;
    let mut runner = Runner {
        child: runtime::launch_command(args)?.spawn()?,
        args,
    };
    let (tx, rx) = mpsc::sync_channel(256);
    if let Some(pipe) = runner.child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let event = serde_json::from_str::<Event>(&line).unwrap_or(Event::Log {
                    message: line,
                    raw: false,
                });
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(pipe) = runner.child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if tx
                    .send(Event::Log {
                        message: line,
                        raw: false,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    let interactive =
        !args.plain && std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    let mut terminal = if interactive {
        Some(ratatui::try_init()?)
    } else {
        None
    };
    let reset = TerminalReset(interactive);
    let mut last_stamp = 0.;
    let mut last_stop = Instant::now() - Duration::from_secs(1);
    let mut force = false;
    let exit = loop {
        for event in rx.try_iter().take(128) {
            if !interactive {
                plain(&event, args.raw, &mut last_stamp);
            }
            app.apply(event);
        }
        if signal.swap(false, Ordering::Relaxed) {
            force = app.stopping;
            app.stopping = true;
        }
        if let Some(terminal) = &mut terminal {
            terminal.draw(|f| {
                viewport(&mut app, f.area().height);
                ui::render(f, &app)
            })?;
            if event::poll(Duration::from_millis(80))? {
                match event::read()? {
                    TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        let quit = key.code == KeyCode::Char('q')
                            || (key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL));
                        if quit {
                            force = app.stopping;
                            app.stopping = true;
                        } else if !control_key(&mut app, args, key.code, &tx) {
                            key_event(&mut app, key);
                        }
                    }
                    TermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }
        } else {
            thread::sleep(Duration::from_millis(80));
        }
        if app.stopping && last_stop.elapsed() >= Duration::from_millis(500) {
            runtime::stop_unit(args, force);
            last_stop = Instant::now();
        }
        if let Some(status) = runner.child.try_wait()? {
            // Drain final shutdown events before leaving the alternate screen.
            for event in rx.try_iter() {
                if !interactive {
                    plain(&event, args.raw, &mut last_stamp);
                }
                app.apply(event);
            }
            break status.code().unwrap_or(1);
        }
    };
    drop(reset);
    let total = app.metrics.totals();
    if exit != 0 && !app.stopping {
        for line in app.logs.iter().rev().take(4).rev() {
            eprintln!("{line}");
        }
    }
    println!(
        "Strata stopped · {} requests · {} input · {} output · {:.1} tok/s active average",
        total.requests,
        ui::commas(total.input),
        ui::commas(total.output),
        total.tps
    );
    Ok(if app.stopping { 0 } else { exit })
}

fn plain(event: &Event, raw: bool, stamp: &mut f64) {
    match event {
        Event::Log {
            message,
            raw: is_raw,
        } => {
            if !is_raw
                || raw
                || ["error", "warning", "failed"]
                    .iter()
                    .any(|s| message.to_lowercase().contains(s))
            {
                println!(":: {}", model::sanitize(message));
            }
        }
        Event::Metrics(metrics) => {
            if let Some(rows) = metrics.data["requests"].as_array() {
                for row in rows.iter().rev() {
                    let at = number(row, "/time");
                    if at > *stamp {
                        *stamp = at;
                        println!(
                            ":: {} input · {} cached · {} output · {:.1} tok/s · {:.1}s · {}",
                            ui::commas(number(row, "/prompt_tokens") as u64),
                            ui::commas(number(row, "/reused") as u64),
                            ui::commas(number(row, "/output_tokens") as u64),
                            number(row, "/decode_tok_s"),
                            number(row, "/duration_s"),
                            text(row, "/finish")
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn key_event(app: &mut App, key: KeyEvent) {
    if app.theme_picker.is_some() {
        app.pending_g = None;
        key_action(app, key.code);
        return;
    }
    if !app.help && key.modifiers.contains(KeyModifiers::CONTROL) {
        let page = if app.process_focus {
            app.process_rows.max(1)
        } else {
            app.page_rows
        };
        let (direction, count) = match key.code {
            KeyCode::Char('d') => (KeyCode::Down, page.div_ceil(2)),
            KeyCode::Char('u') => (KeyCode::Up, page.div_ceil(2)),
            KeyCode::Char('f') => (KeyCode::Down, page),
            KeyCode::Char('b') => (KeyCode::Up, page),
            _ => {
                key_action(app, key.code);
                return;
            }
        };
        app.pending_g = None;
        for _ in 0..count {
            key_action(app, direction);
        }
        return;
    }
    if !app.help && key.code == KeyCode::Char('g') {
        if app
            .pending_g
            .take()
            .is_some_and(|at| at.elapsed() < Duration::from_millis(700))
        {
            key_action(app, KeyCode::Home);
        } else {
            app.pending_g = Some(Instant::now());
        }
        return;
    }
    app.pending_g = None;
    key_action(app, key.code);
}

pub fn key_action(app: &mut App, key: KeyCode) {
    if let Some(original) = app.theme_picker {
        let index = theme::Theme::ALL
            .iter()
            .position(|t| *t == app.theme)
            .unwrap_or(0);
        match key {
            KeyCode::Char('j' | 'l') | KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                app.theme = theme::Theme::ALL[(index + 1) % theme::Theme::ALL.len()];
                app.theme_notice = None;
            }
            KeyCode::Char('k' | 'h') | KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                app.theme = theme::Theme::ALL
                    [(index + theme::Theme::ALL.len() - 1) % theme::Theme::ALL.len()];
                app.theme_notice = None;
            }
            KeyCode::Home | KeyCode::Char('g') => app.theme = theme::Theme::Strata,
            KeyCode::End | KeyCode::Char('G') => app.theme = *theme::Theme::ALL.last().unwrap(),
            KeyCode::Esc | KeyCode::Char('t') => {
                app.theme = original;
                app.theme_picker = None;
                app.theme_notice = None;
            }
            KeyCode::Enter => {
                if let Some(path) = &app.theme_path
                    && let Err(e) = if app.theme_profile.is_some() {
                        theme::save_profile(path, app.theme, app.theme_profile.as_deref())
                    } else {
                        theme::save(path, app.theme)
                    }
                {
                    app.theme_notice = Some(format!("Could not save: {e}. Esc cancels."));
                    return;
                }
                app.theme_picker = None;
                app.theme_notice = None;
            }
            _ => {}
        }
        return;
    }
    if app.help {
        if matches!(key, KeyCode::Esc | KeyCode::Char('?')) {
            app.help = false;
        }
        return;
    }
    if key == KeyCode::Esc && app.control_notice.is_some() {
        app.control_notice = None;
        return;
    }
    if key == KeyCode::Char('a') {
        app.age_seconds = !app.age_seconds;
        app.refresh_ages();
        return;
    }
    if key == KeyCode::Char('t') {
        app.theme_picker = Some(app.theme);
        return;
    }
    if key == KeyCode::Char('p') && app.process_rows > 0 {
        app.process_focus = !app.process_focus;
        return;
    }
    if app.process_focus {
        let end = app
            .hardware
            .processes
            .len()
            .saturating_sub(app.process_rows);
        match key {
            KeyCode::Down | KeyCode::Char('j') => {
                app.process_offset = (app.process_offset + 1).min(end);
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.process_offset = app.process_offset.saturating_sub(1);
                return;
            }
            KeyCode::Home => {
                app.process_offset = 0;
                return;
            }
            KeyCode::End | KeyCode::Char('G') => {
                app.process_offset = end;
                return;
            }
            KeyCode::Esc => {
                app.process_focus = false;
                return;
            }
            KeyCode::PageDown => {
                app.process_offset = (app.process_offset + app.process_rows).min(end);
                return;
            }
            KeyCode::PageUp => {
                app.process_offset = app.process_offset.saturating_sub(app.process_rows);
                return;
            }
            KeyCode::Char('?') => {
                app.help = true;
                return;
            }
            _ => app.process_focus = false,
        }
    }
    match key {
        KeyCode::Left | KeyCode::Char('h') => {
            app.view = match app.view {
                View::Policy => View::Logs,
                View::Logs => View::Requests,
                _ => View::Overview,
            };
            app.scroll = 0;
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.view = match app.view {
                View::Overview => View::Requests,
                View::Requests => View::Logs,
                _ => View::Policy,
            };
            app.scroll = 0;
        }
        KeyCode::Enter if app.view == View::Overview => {
            app.view = View::Requests;
        }
        KeyCode::Char('1') => {
            app.view = View::Overview;
            app.scroll = 0;
        }
        KeyCode::Char('2') => {
            app.view = View::Requests;
            app.scroll = 0;
        }
        KeyCode::Char('4') => {
            app.view = View::Policy;
            app.scroll = 0;
        }
        KeyCode::Char('3') => {
            app.view = View::Logs;
            app.scroll = 0;
        }
        KeyCode::Tab => {
            app.view = match app.view {
                View::Overview => View::Requests,
                View::Requests => View::Logs,
                View::Logs => View::Policy,
                View::Policy => View::Overview,
            };
            app.scroll = 0;
        }
        KeyCode::Char('?') => app.help = true,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.view == View::Logs {
                app.scroll = app.scroll.saturating_add(1).min(app.logs.len());
            } else {
                app.scroll = app.scroll.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.view == View::Logs {
                app.scroll = app.scroll.saturating_sub(1);
            } else {
                app.scroll = (app.scroll + 1).min(app.list_len().saturating_sub(1));
            }
        }
        KeyCode::PageUp => {
            for _ in 0..app.page_rows {
                key_action(app, KeyCode::Up);
            }
        }
        KeyCode::PageDown => {
            for _ in 0..app.page_rows {
                key_action(app, KeyCode::Down);
            }
        }
        KeyCode::Home => {
            app.scroll = if app.view == View::Logs {
                app.logs.len().saturating_sub(1)
            } else {
                0
            };
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.scroll = if app.view == View::Logs {
                0
            } else {
                app.list_len().saturating_sub(1)
            };
        }
        KeyCode::Char('b') => {
            let url = format!("http://{}", app.address);
            thread::spawn(move || {
                let _ = Command::new("xdg-open")
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            });
        }
        _ => {}
    }
}

pub fn demo(args: &Args) -> Result<i32> {
    let mut app = model::demo();
    app.step_mib = args.step_mib;
    app.age_seconds = args.age_seconds;
    app.min_free_mib = args.min_free_mib;
    app.grow_below_mib = args.grow_below_mib;
    configure_theme(&mut app, args);
    if let Some(size) = &args.snapshot {
        let (w, h) = size
            .split_once('x')
            .ok_or_else(|| anyhow::anyhow!("snapshot size must be WIDTHxHEIGHT"))?;
        let (w, h): (u16, u16) = (w.parse()?, h.parse()?);
        anyhow::ensure!(w <= 500 && h <= 200, "snapshot is limited to 500×200");
        print!("{}", ui::snapshot(&app, w, h, args.cells)?);
        return Ok(0);
    }
    let signal = runtime::signals()?;
    let mut terminal = ratatui::try_init()?;
    let _reset = TerminalReset(true);
    while !signal.load(Ordering::Relaxed) {
        terminal.draw(|f| {
            viewport(&mut app, f.area().height);
            ui::render(f, &app)
        })?;
        if event::poll(Duration::from_millis(100))?
            && let TermEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('q')
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                break;
            }
            key_event(&mut app, key);
        }
    }
    Ok(0)
}

fn configure_theme(app: &mut App, args: &Args) {
    let path = theme::preferences_path();
    app.theme = if let Some(theme) = args.theme {
        theme
    } else if args.snapshot.is_some() || args.no_config {
        theme::Theme::default()
    } else {
        theme::load(&path).unwrap_or_else(|error| {
            app.theme_notice = Some(format!("Saved theme unavailable: {error}. Using Strata."));
            theme::Theme::default()
        })
    };
    if !app.demo {
        app.theme_profile = args.profile.clone();
        app.theme_path = Some(if args.settings_path.as_os_str().is_empty() {
            crate::config::default_path()
        } else {
            args.settings_path.clone()
        });
    }
}

fn viewport(app: &mut App, height: u16) {
    app.page_rows = ui::visible_rows(app, height);
    app.process_rows = ui::process_height(height)
        .min(app.hardware.processes.len().max(3) as u16 + 3)
        .saturating_sub(3) as usize;
    if app.process_rows == 0 {
        app.process_focus = false;
    }
    app.process_offset = app.process_offset.min(
        app.hardware
            .processes
            .len()
            .saturating_sub(app.process_rows),
    );
}

fn control_key(app: &mut App, args: &Args, key: KeyCode, tx: &mpsc::SyncSender<Event>) -> bool {
    use crate::control::{self, Command};
    if app.attached || app.demo || app.help || app.theme_picker.is_some() {
        return false;
    }
    let command = if let Some(preview) = &app.reload_preview {
        match key {
            KeyCode::Esc => {
                app.reload_preview = None;
                return true;
            }
            KeyCode::Enter => match serde_json::from_value(preview["proposed"].clone()) {
                Ok(effective) => Command::Reload(effective),
                Err(_) => return true,
            },
            _ => return true,
        }
    } else {
        match key {
            KeyCode::Char(' ') => {
                Command::PauseGrowth(!app.supervisor.as_ref().is_some_and(|s| s.growth_paused))
            }
            KeyCode::Char('r') => Command::Resume,
            KeyCode::Char('c') => Command::PreviewReload,
            _ => return false,
        }
    };
    let preview = matches!(command, Command::PreviewReload);
    if app.control_pending {
        return true;
    }
    app.control_pending = true;
    app.control_notice = Some("Waiting for worker…".into());
    app.reload_preview = None;
    let args = args.clone();
    let tx = tx.clone();
    thread::spawn(move || {
        let result = control::call(&args, command).map_err(|e| e.to_string());
        let _ = tx.send(Event::ControlReply { preview, result });
    });
    true
}

pub fn attach(args: &Args) -> Result<i32> {
    use crate::{control, hardware::Collector};
    use std::sync::{Arc, atomic::AtomicBool};
    let managed = control::socket_path(args).exists();
    let mut app = App::new("Attached server".into(), args.port.unwrap_or(8080));
    app.attached = true;
    app.age_seconds = args.age_seconds;
    configure_theme(&mut app, args);
    app.theme_path = None;
    let (api, gpu) = if managed {
        let state: control::Snapshot =
            serde_json::from_value(control::call(args, control::Command::Status)?)?;
        app.address = format!("127.0.0.1:{}", state.port);
        // the worker read the model config; its rates ride the config path it reports
        app.pricing = model::pricing_at(&state.config_path);
        app.apply(Event::Supervisor(Box::new(state)));
        (None, 0)
    } else {
        anyhow::ensure!(
            args.model.is_some(),
            "choose the running server's model config, or the managed worker's --unit"
        );
        let mut resolved = args.clone();
        let model = runtime::read_config(args)?;
        crate::config::resolve_model(&mut resolved, &model)?;
        let port = args
            .port
            .unwrap_or_else(|| model["port"].as_u64().unwrap_or(8080) as u16);
        app.address = format!("127.0.0.1:{port}");
        app.policy = "Read-only attachment · external policy unknown".into();
        app.pricing = crate::model::pricing(&model);
        (
            Some(runtime::Api::new(
                port,
                model["api_key"].as_str().unwrap_or(""),
            )?),
            resolved.gpu.as_deref().unwrap_or("0").parse()?,
        )
    };
    let (tx, rx) = mpsc::sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let settings = args.clone();
    thread::spawn(move || {
        let mut collector = Collector::new(gpu);
        collector.external = true;
        while !flag.load(Ordering::Relaxed) {
            let result = (|| -> Result<Vec<Event>> {
                if managed {
                    let snapshot: control::Snapshot = serde_json::from_value(control::call(
                        &settings,
                        control::Command::Status,
                    )?)?;
                    let mut events = Vec::new();
                    if control::epoch() - snapshot.metrics_at < 4.
                        && let Some(metrics) = &snapshot.metrics
                    {
                        events.push(Event::Metrics(metrics.clone()));
                    }
                    if let Some(hardware) = &snapshot.hardware {
                        events.push(Event::Hardware(Box::new(hardware.clone())));
                    }
                    events.push(Event::Supervisor(Box::new(snapshot)));
                    Ok(events)
                } else {
                    let api = api.as_ref().unwrap();
                    Ok(vec![
                        Event::Metrics(model::Metrics {
                            status: api.get("/v1/status")?,
                            data: api.get("/metrics?requests=all")?,
                        }),
                        Event::Hardware(Box::new(collector.sample())),
                    ])
                }
            })();
            match result {
                Ok(events) => {
                    for event in events {
                        if tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    if tx
                        .send(Event::Log {
                            message: format!("Attachment: {error}"),
                            raw: false,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            for _ in 0..10 {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    let signal = runtime::signals()?;
    let interactive =
        !args.plain && std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    let mut terminal = if interactive {
        Some(ratatui::try_init()?)
    } else {
        None
    };
    let _reset = TerminalReset(interactive);
    let mut stamp = 0.;
    loop {
        for event in rx.try_iter().take(128) {
            if !interactive {
                plain(&event, args.raw, &mut stamp);
            }
            app.apply(event);
        }
        if signal.swap(false, Ordering::Relaxed) {
            break;
        }
        if let Some(terminal) = &mut terminal {
            terminal.draw(|f| {
                viewport(&mut app, f.area().height);
                ui::render(f, &app)
            })?;
            if event::poll(Duration::from_millis(80))?
                && let TermEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if key.code == KeyCode::Char('q')
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    break;
                }
                key_event(&mut app, key);
            }
        } else {
            thread::sleep(Duration::from_millis(80));
        }
    }
    stop.store(true, Ordering::Relaxed);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(app: &mut App, code: KeyCode) {
        key_event(app, KeyEvent::new(code, KeyModifiers::NONE));
    }
    #[test]
    fn theme_preview_is_modal_and_escape_restores_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let mut app = model::demo();
        app.theme_path = Some(path.clone());
        app.process_focus = true;
        key(&mut app, KeyCode::Char('t'));
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.theme, theme::Theme::Ember);
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.theme, theme::Theme::Monochrome);
        assert_eq!(app.view, View::Overview);
        assert_eq!(app.scroll, 0);
        assert_eq!(app.process_offset, 0);
        assert!(!path.exists());
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.theme, theme::Theme::Strata);
        assert!(app.theme_picker.is_none());
        assert!(!path.exists());
        key(&mut app, KeyCode::Char('t'));
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.theme, theme::Theme::Everforest);
        key(&mut app, KeyCode::Enter);
        assert!(app.theme_picker.is_none());
        assert_eq!(theme::load(&path).unwrap(), theme::Theme::Everforest);
    }

    #[test]
    fn preview_never_saves_preferences_and_accepts_cli_palette() {
        let args =
            <Args as clap::Parser>::parse_from(["strata-wrap", "--preview", "--theme", "ember"]);
        assert!(args.demo);
        let mut app = model::demo();
        configure_theme(&mut app, &args);
        assert_eq!(app.theme, theme::Theme::Ember);
        assert!(app.theme_path.is_none());
        key(&mut app, KeyCode::Char('t'));
        key(&mut app, KeyCode::Char('l'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.theme, theme::Theme::Monochrome);
        assert!(app.theme_picker.is_none());
    }

    #[test]
    fn theme_picker_reaches_new_palettes_and_tracks_the_catalog_end() {
        let mut app = model::demo();
        key(&mut app, KeyCode::Char('t'));
        key(&mut app, KeyCode::Char('G'));
        assert_eq!(app.theme, theme::Theme::Everforest);
        key(&mut app, KeyCode::Char('h'));
        assert_eq!(app.theme, theme::Theme::RosePine);
        key(&mut app, KeyCode::Char('h'));
        assert_eq!(app.theme, theme::Theme::AyuDark);
        key(&mut app, KeyCode::Home);
        assert_eq!(app.theme, theme::Theme::Strata);
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.theme, theme::Theme::Everforest);
        key(&mut app, KeyCode::Enter);
        assert!(app.theme_picker.is_none());
    }

    #[test]
    fn save_failure_keeps_picker_open_and_original_file_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(&path, "invalid json").unwrap();
        let mut app = model::demo();
        app.theme_path = Some(path.clone());
        key(&mut app, KeyCode::Char('t'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);
        assert!(app.theme_picker.is_some());
        assert!(
            app.theme_notice
                .as_deref()
                .unwrap()
                .starts_with("Could not save:")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid json");
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.theme, theme::Theme::Strata);
    }
    #[test]
    fn vim_views_selection_and_gg_g() {
        let mut app = model::demo();
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.view, View::Requests);
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.scroll, 1);
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.scroll, 0);
        key(&mut app, KeyCode::Char('G'));
        assert_eq!(app.scroll, 7);
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.scroll, 7);
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.scroll, 0);
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.view, View::Logs);
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.scroll, 1);
        key(&mut app, KeyCode::Char('G'));
        assert_eq!(app.scroll, 0);
        key(&mut app, KeyCode::Char('h'));
        assert_eq!(app.view, View::Requests);
    }
    #[test]
    fn control_page_navigation_is_bounded() {
        let mut app = model::demo();
        app.view = View::Requests;
        app.page_rows = 6;
        key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.scroll, 3);
        key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.scroll, 0);
        key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.scroll, 6);
        key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn gpu_process_focus_scrolls_with_vim_and_clears_when_hidden() {
        let mut app = model::demo();
        app.process_rows = 3;
        key(&mut app, KeyCode::Char('p'));
        assert!(app.process_focus);
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.process_offset, 1);
        key(&mut app, KeyCode::Char('G'));
        assert_eq!(app.process_offset, 2);
        key(&mut app, KeyCode::Char('g'));
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.process_offset, 0);
        viewport(&mut app, 23);
        assert!(!app.process_focus);
        assert_eq!(app.process_rows, 0);
    }
}
