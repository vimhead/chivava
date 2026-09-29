mod app;
mod audio;
mod bundled_themes;
mod chart_layout;
mod chart_plot;
mod chart_text;
mod cli;
mod content;
mod engine;
mod kitty_graphics;
mod licenses;
mod managed_settings;
mod result_chart;
mod selection;
mod settings;
mod statistics;
mod statistics_export;
mod syntax;
mod terminal;
mod theme;
mod typing_view;
mod ui;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};

use app::{Application, ApplicationInput, InputFeedback};
use audio::KeystrokeAudio;
use content::Corpus;
use settings::Configuration;
use statistics_export::StatisticsExport;
use terminal::InlineTerminal;
use theme::{Theme, ThemeSelection};

#[derive(Parser)]
#[command(
    version = option_env!("CHIVAVA_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
    about,
    args_conflicts_with_subcommands = true,
    after_help = concat!(
        "Run without a command to start typing. All content works offline.\n",
        "F1 shows keys; F2 opens settings; F5 restarts; Ctrl-C quits.\n",
        "Esc enters normal mode while typing, or backs out of menus.\n\n",
        "Use config set --help for settings and values.\n",
        "Config: CHIVAVA_CONFIG_DIR overrides $XDG_CONFIG_HOME/chivava or ~/.config/chivava.\n",
        "Charts require Kitty, Ghostty or WezTerm with pixel dimensions, outside tmux/screen."
    )
)]
struct Arguments {
    #[command(subcommand)]
    command: Option<cli::Command>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Export timing/counts after each test (overwrites FILE; no typed text)"
    )]
    export_stats: Option<PathBuf>,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    if let Some(command) = arguments.command {
        return command.execute();
    }
    let directory = settings::resolve_config_directory()?;
    let configuration = Configuration::load(&directory)?;
    let services =
        (io::stdin().is_terminal() && io::stdout().is_terminal()).then(|| TerminalServices {
            audio: KeystrokeAudio::start(configuration.settings.sound),
            statistics_export: StatisticsExport::create(arguments.export_stats),
        });
    let mut themes = Theme::load_bundled()?;
    let home_directory = std::env::var_os("HOME").map(PathBuf::from);
    Theme::load_selected(
        &mut themes,
        ThemeSelection {
            selector: &configuration.settings.theme,
            config_directory: &directory,
            home_directory: home_directory.as_deref(),
        },
    )?;
    let services =
        services.context("chivava needs an interactive terminal; use --help for CLI options")?;
    let corpus = Corpus::load(&directory)?;
    let mut application = Application::create(ApplicationInput {
        configuration,
        directory,
        corpus,
        themes,
    })?;
    run_terminal(&mut application, services)
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        crossterm::style::SetAttribute(crossterm::style::Attribute::Reset),
        crossterm::style::ResetColor,
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    );
}

struct TerminalServices {
    audio: io::Result<KeystrokeAudio>,
    statistics_export: StatisticsExport,
}

fn run_terminal(application: &mut Application, services: TerminalServices) -> Result<()> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |information| {
        restore_terminal();
        previous_hook(information);
    }));
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnableBracketedPaste)?;
    let mut terminal = InlineTerminal::create(&ui::View {
        application,
        now: Duration::ZERO,
    })?;
    let result = drive_terminal(
        application,
        TerminalOutput {
            terminal: &mut terminal,
            services,
        },
    );
    let cleanup = terminal.finish(application.screen);
    result.and(cleanup.map_err(Into::into))
}

struct TerminalOutput<'a> {
    terminal: &'a mut InlineTerminal,
    services: TerminalServices,
}

fn drive_terminal(application: &mut Application, output: TerminalOutput<'_>) -> Result<()> {
    let TerminalOutput {
        terminal,
        services: TerminalServices {
            audio,
            mut statistics_export,
        },
    } = output;
    let clock = Instant::now();
    let mut audio = match audio {
        Ok(audio) => Some(audio),
        Err(_) => {
            application.notice = Some("Sound unavailable; typing continues.".into());
            None
        }
    };
    while !application.should_quit {
        if let Some(audio) = &mut audio {
            audio.configure(application.configuration.settings.sound);
            if let Some(notice) = audio.take_notice()
                && application.notice.is_none()
            {
                application.notice = Some(notice.into());
            }
        }
        application.tick(clock.elapsed());
        if let Err(error) = statistics_export.write_finished_session(&application.session) {
            application.notice = Some(format!("{error:#}"));
        }
        terminal.draw(&ui::View {
            application,
            now: clock.elapsed(),
        })?;
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) => {
                    match application.handle_key_with_feedback(key, clock.elapsed()) {
                        Ok(InputFeedback::Keystroke) => {
                            if let Some(audio) = &audio {
                                audio.play_click(key);
                            }
                        }
                        Ok(InputFeedback::Silent) => {}
                        Err(error) => {
                            if application.should_quit {
                                return Err(error);
                            }
                            application.notice = Some(format!("Error: {error:#}"));
                        }
                    }
                }
                Event::Paste(_) => {
                    application.notice = Some("Paste disabled for typing practice".into())
                }
                _ => {}
            }
        }
    }
    Ok(())
}
