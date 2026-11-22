//! Lifecycle wiring. Slint stays in tg-ui; TDLib and JSON stay in tg-telegram.
// Release builds are GUI applications: no console window opens next to the client.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod demo;
mod files;
mod options;
mod recovery;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
};
use tg_core::{Command, CommandError, CommandKind, Event, SecurityWarning};
use tg_telegram::{Engine, EngineParameters};
use tg_ui::{EventSink, Ui};
fn main() -> std::process::ExitCode {
    // Before any window, engine or library: DLL search order, image load policy, and a process
    // DACL that keeps same-user programs out of this process's memory (SECURITY.md, T3).
    let _hardening = tg_platform::harden_process();
    // Started from a terminal: print there. Started from Explorer: errors go to a message box.
    let console = tg_platform::attach_parent_console();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // Errors produced here are fixed application text, never credential/native payloads.
        Err(error) => {
            if console {
                eprintln!("{error}");
            } else {
                tg_platform::show_error(&error.to_string());
            }
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = options::Options::parse(std::env::args().skip(1))?;
    if options.help {
        println!(
            "veygram [--demo | --demo-text-only] [--theme dark|oled] [--snapshot <png>] [--demo-scroll <seconds>] [--demo-voice <seconds>] [--demo-round <seconds>] [--demo-stickers] [--demo-video <seconds>] [--check <json>]\n--demo runs synthetic conversations without TDLib, credentials or key files."
        );
        return Ok(());
    }
    if options.demo {
        let media_path = std::thread::spawn(demo::prepare_media)
            .join()
            .map_err(|_| "Demo media worker failed")??;
        let backend = Rc::new(RefCell::new(demo::Demo::new(
            media_path.clone(),
            options.demo_text_only,
        )));
        let commands = backend.clone();
        // Demo never resolves TG_DATA_DIR. An explicitly configured emoji font is optional.
        let font = std::thread::spawn(|| config::emoji_font(None))
            .join()
            .map_err(|_| "Font loader failed")??;
        let ui = Ui::new(
            tg_ui::Options {
                oled: options.oled,
                demo: true,
                snapshot: options.snapshot,
                emoji_font: font,
                demo_scroll: options.demo_scroll,
                demo_video: options.demo_video,
                demo_voice: options.demo_voice,
                demo_round: options.demo_round,
                demo_stickers: options.demo_stickers,
                check: options.check.clone(),
                // Demo keeps the system scale so its snapshots stay comparable.
                interface_scale: tg_core::SCALE_AUTO,
            },
            move |command| commands.borrow_mut().command(command),
        )?;
        backend.borrow_mut().sink = Some(ui.event_sink());
        backend.borrow_mut().seed();
        let notifications = attach_notifications(&ui);
        let player = attach_video(&ui);
        let _files = files::attach(&ui);
        backend.borrow_mut().schedule_incoming();
        let result = ui.run();
        drop(player);
        drop(notifications);
        let _ = std::thread::spawn(move || demo::cleanup_media(&media_path)).join();
        return result;
    }
    // Pre-window configuration reads are also off the UI thread. No keys or native engine yet.
    let (credentials, directory, font, scale) = std::thread::spawn(|| {
        let credentials = config::credentials()?;
        let directory = config::plain_directory(
            tg_security::data_dir().map_err(|_| "Invalid application data directory")?,
        );
        let font = config::emoji_font(Some(&directory))?;
        let scale = config::interface_scale(&directory);
        Ok::<_, &'static str>((credentials, directory, font, scale))
    })
    .join()
    .map_err(|_| "Configuration worker failed")??;
    // The worker has ended and nothing else runs yet: the toolkit reads this with its first window.
    if let Some(factor) = tg_core::scale_factor(scale) {
        tg_platform::set_interface_scale(factor);
    }
    let engine: Arc<Mutex<Option<Engine>>> = Arc::default();
    let dispatch = engine.clone();
    // The lifecycle worker owns the receiver: the close token and the recovery choices.
    let (control, controls) = mpsc::sync_channel::<Control>(2);
    let choice = control.clone();
    let ui = Ui::new(
        tg_ui::Options {
            oled: options.oled,
            demo: false,
            snapshot: options.snapshot,
            emoji_font: font,
            demo_scroll: None,
            demo_video: None,
            demo_voice: None,
            demo_round: None,
            demo_stickers: false,
            check: options.check.clone(),
            interface_scale: scale,
        },
        move |command| {
            // Recovery choices go to the lifecycle worker; the engine is not running then.
            let recovery = match command {
                Command::RetryLocalData => Some(Control::Retry),
                Command::ResetLocalData => Some(Control::Reset),
                _ => None,
            };
            if let Some(recovery) = recovery {
                return choice
                    .try_send(recovery)
                    .map_err(|_| failure("Veygram is busy. Please try again."));
            }
            let slot = dispatch
                .try_lock()
                .map_err(|_| failure("The engine is busy. Please try again."))?;
            slot.as_ref()
                .ok_or_else(|| failure("The engine is still starting."))?
                .send(command)
        },
    )?;
    let notifications = attach_notifications(&ui);
    let player = attach_video(&ui);
    let _files = files::attach(&ui);
    // A new interface scale is stored now and applied by a restart after a clean shutdown.
    let restart = Rc::new(std::cell::Cell::new(false));
    let requested = restart.clone();
    let scale_directory = directory.clone();
    ui.on_interface_scale(move |value| {
        config::save_interface_scale(&scale_directory, value)
            .map_err(|_| failure("Could not save the interface scale."))?;
        requested.set(true);
        Ok(())
    });
    let sink = ui.event_sink();
    // Authorization can arrive before Engine::start returns. Hold delivery until
    // the handle is installed, otherwise the UI's default QR request can race it.
    sink.pause();
    let worker = std::thread::Builder::new()
        .name("tg-app-lifecycle".into())
        .spawn(move || lifecycle(&directory, Some(credentials), &engine, &sink, &controls))
        .map_err(|_| "Could not start the lifecycle worker")?;
    let result = ui.run();
    drop(player);
    drop(notifications);
    // Blocks only until the worker reads it; fails at once if the worker has already ended.
    let _ = control.send(Control::Close);
    let shutdown = worker.join().map_err(|_| "Lifecycle worker failed")?;
    result?;
    shutdown?;
    // Only after TDLib has closed its database, so the new process can open it.
    if restart.get() {
        let exe = std::env::current_exe().map_err(|_| "Could not find Veygram to restart it")?;
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .spawn()
            .map_err(|_| "Could not restart Veygram")?;
    }
    Ok(())
}
/// Messages to the lifecycle worker.
enum Control {
    Close,
    Retry,
    Reset,
}
enum StartError {
    /// The key file cannot be unlocked here; the user decides what happens next.
    Locked,
    Failed(CommandError),
}

/// Starts the engine, and when the database key cannot be unlocked offers the recovery screen
/// until the user retries successfully, starts fresh, or closes the window. Never on the UI thread.
fn lifecycle(
    directory: &Path,
    mut credentials: Option<config::Credentials>,
    engine: &Arc<Mutex<Option<Engine>>>,
    sink: &EventSink,
    controls: &mpsc::Receiver<Control>,
) -> Result<(), &'static str> {
    loop {
        // The first attempt uses the credentials read before the window opened.
        let attempt = match credentials.take() {
            Some(credentials) => Ok(credentials),
            None => config::credentials().map_err(|error| StartError::Failed(failure(error))),
        }
        .and_then(|credentials| start_engine(directory, credentials, sink));
        match attempt {
            Ok(started) => {
                if let Ok(mut slot) = engine.lock() {
                    *slot = Some(started);
                }
                sink.resume();
                // Recovery choices are meaningless once the engine runs.
                while let Ok(Control::Retry | Control::Reset) = controls.recv() {}
                let owned = engine.lock().ok().and_then(|mut slot| slot.take());
                if let Some(owned) = owned {
                    owned.shutdown().map_err(|_| "Engine shutdown failed")?;
                }
                return Ok(());
            }
            Err(StartError::Locked) => {
                sink.post(Event::LocalDataLocked);
                sink.resume();
                match controls.recv() {
                    Ok(Control::Retry) => sink.pause(),
                    Ok(Control::Reset) => match recovery::move_aside(directory) {
                        Ok(_) => sink.pause(),
                        // Stay on the recovery screen; nothing was moved.
                        Err(_) => startup_error(
                            sink,
                            failure(
                                "Could not move the old data aside. Close programs that use it and try again.",
                            ),
                        ),
                    },
                    Ok(Control::Close) | Err(_) => return Ok(()),
                }
            }
            Err(StartError::Failed(error)) => {
                startup_error(sink, error);
                sink.resume();
                while let Ok(Control::Retry | Control::Reset) = controls.recv() {}
                return Ok(());
            }
        }
    }
}

fn start_engine(
    directory: &Path,
    credentials: config::Credentials,
    sink: &EventSink,
) -> Result<Engine, StartError> {
    let protection = tg_security::load_or_create(directory).map_err(|error| {
        if error.is_unreadable_key() {
            StartError::Locked
        } else {
            StartError::Failed(failure(
                "Could not unlock the database key. Existing key files were preserved.",
            ))
        }
    })?;
    if !protection.tpm_protected {
        sink.post(Event::SecurityWarning(SecurityWarning::TpmUnavailable));
    }
    let events = sink.clone();
    let parameters = EngineParameters {
        api_id: credentials.id,
        api_hash: credentials.hash,
        database_encryption_key: *protection.key.expose(),
        data_directory: PathBuf::from(directory),
    };
    drop(protection);
    Engine::start(parameters, move |event| events.post(event)).map_err(StartError::Failed)
}

/// Desktop notifications are optional: without a tray (restricted sessions, sandboxes)
/// the client keeps running and shows a warning instead of exiting.
fn attach_notifications(ui: &Ui) -> Option<Arc<tg_platform::Notifications>> {
    let events = ui.event_sink();
    let started = std::thread::spawn(move || {
        tg_platform::Notifications::start(move |chat| {
            events.post(Event::NotificationActivated(chat))
        })
    })
    .join();
    let Ok(Ok(service)) = started else {
        startup_error(
            &ui.event_sink(),
            failure("Desktop notifications are unavailable in this session."),
        );
        return None;
    };
    let service = Arc::new(service);
    let send = service.clone();
    ui.on_notification(move |notification| {
        send.show(notification)
            .map_err(|_| failure("Could not show a desktop notification."))
    });
    Some(service)
}

/// The video player is optional too: without Media Foundation the client keeps running and
/// the player reports that playback is unavailable.
fn attach_video(ui: &Ui) -> Option<Rc<tg_platform::Player>> {
    let events = ui.event_sink();
    let frames = events.clone();
    let player = Rc::new(
        tg_platform::Player::start_with_frames(
            move |state| events.post(Event::VideoState(state)),
            move |frame| frames.post(Event::RoundFrame(frame)),
        )
        .ok()?,
    );
    let requests = player.clone();
    ui.on_video(move |request| {
        requests
            .request(request)
            .map_err(|_| failure("Could not play the video."))
    });
    Some(player)
}

fn failure(message: &str) -> CommandError {
    CommandError {
        code: -4,
        message: message.to_owned(),
    }
}
fn startup_error(sink: &EventSink, error: CommandError) {
    sink.post(Event::CommandFailed {
        command: CommandKind::Initialize,
        error,
    });
}
