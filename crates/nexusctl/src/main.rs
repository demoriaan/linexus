mod actions_cmd;
mod config_cmd;
mod daemon_cmd;
mod device;
mod doctor;
mod fix;
mod gnome_cmd;
mod metrics;
mod pack_cmd;
mod preview;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "linexusctl",
    version,
    about = "Control and diagnose the Corsair iCUE Nexus touch strip on Linux"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check device access, services, config and helper tools.
    Doctor {
        /// Apply safe fixes; print the commands for those that need root.
        #[arg(long)]
        fix: bool,
        /// With --fix: show what would change without changing anything.
        #[arg(long, requires = "fix")]
        dry_run: bool,
        /// Also list interfaces, services, sockets and sensors.
        #[arg(long, short)]
        verbose: bool,
    },
    /// Print the udev rule that lets your user open the strip.
    PrintUdevRule,
    /// Print the systemd system unit for the privileged input helper.
    PrintInputHelperService {
        /// UID allowed to use the helper socket; the current user by default.
        #[arg(long)]
        allow_uid: Option<u32>,
        /// Helper binary path.
        #[arg(long, default_value = fix::INPUT_HELPER_BINARY)]
        binary_path: PathBuf,
    },
    /// Read the firmware version from the strip.
    Info,
    /// Set the strip brightness (0..=100) directly on the device.
    Brightness { percent: u8 },
    /// Clear the strip.
    Blank,
    /// Show a test pattern on the strip.
    Demo {
        #[arg(long, value_enum, default_value_t = preview::Pattern::RgbBars)]
        pattern: preview::Pattern,
    },
    /// Show an image file on the strip.
    Image {
        path: PathBuf,
        #[arg(long, value_enum, default_value_t = device::ImageFitArg::Fill)]
        fit: device::ImageFitArg,
    },
    /// Print touch events and recognized gestures.
    Touch {
        #[arg(default_value_t = 10)]
        seconds: u64,
    },
    /// Print the system readings collected on this machine.
    Sensors,
    /// Check the backends that desktop actions use.
    Actions {
        #[command(subcommand)]
        command: actions_cmd::ActionsCommand,
    },
    /// Run (or dry-run) one desktop action.
    Action {
        #[command(subcommand)]
        command: actions_cmd::ActionCommand,
    },
    /// Talk to the keyboard/text input helper.
    InputHelper {
        /// Input helper socket path.
        #[arg(long, global = true)]
        socket_path: Option<PathBuf>,
        #[command(subcommand)]
        command: actions_cmd::InputHelperCommand,
    },
    /// Create, check, upgrade and inspect the config file.
    Config {
        #[command(subcommand)]
        command: config_cmd::ConfigCommand,
    },
    /// Show or clear the event log.
    Events {
        #[command(subcommand)]
        command: pack_cmd::EventsCommand,
    },
    /// Control the running daemon (linexusd).
    Daemon {
        /// Daemon socket path.
        #[arg(long, global = true)]
        socket_path: Option<PathBuf>,
        #[command(subcommand)]
        command: daemon_cmd::DaemonCommand,
    },
    /// Install the GNOME Shell extension that app-aware profiles need on
    /// GNOME.
    GnomeExtension {
        #[command(subcommand)]
        command: gnome_cmd::GnomeExtensionCommand,
    },
    /// Export, inspect or import shareable .nexuspack files.
    Pack {
        #[command(subcommand)]
        command: pack_cmd::PackCommand,
    },
    /// Render a page, pop-up or test pattern to PNG without the device.
    RenderPreview(preview::RenderPreviewArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Doctor {
            fix,
            dry_run,
            verbose,
        } => {
            if fix {
                fix::run(dry_run)?;
            }
            doctor::run(verbose);
            Ok(())
        }
        Command::PrintUdevRule => {
            print!("{}", nexus_core::udev_rule_text());
            Ok(())
        }
        Command::PrintInputHelperService {
            allow_uid,
            binary_path,
        } => {
            let allow_uid = allow_uid.unwrap_or_else(util::current_uid);
            print!(
                "{}",
                fix::input_helper_service_text(&binary_path, allow_uid)
            );
            Ok(())
        }
        Command::Info => device::info(),
        Command::Brightness { percent } => device::set_brightness(percent),
        Command::Blank => device::blank(),
        Command::Demo { pattern } => {
            device::push_frame(&pattern.frame())?;
            println!("Test pattern shown: {}", pattern.name());
            Ok(())
        }
        Command::Image { path, fit } => device::push_image(&path, fit),
        Command::Touch { seconds } => device::touch(seconds),
        Command::Sensors => {
            metrics::print_metrics(&nexus_sensors::collect_metrics());
            Ok(())
        }
        Command::Actions { command } => actions_cmd::run_actions(command),
        Command::Action { command } => actions_cmd::run_action(command),
        Command::InputHelper {
            socket_path,
            command,
        } => actions_cmd::run_input_helper(command, socket_path),
        Command::Config { command } => config_cmd::run(command),
        Command::Events { command } => pack_cmd::run_events(command),
        Command::Daemon {
            socket_path,
            command,
        } => daemon_cmd::run(command, socket_path),
        Command::GnomeExtension { command } => gnome_cmd::run(command),
        Command::Pack { command } => pack_cmd::run_pack(command),
        Command::RenderPreview(args) => preview::run(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn render_preview_sources_are_exclusive() {
        let parse =
            |args: &[&str]| Cli::try_parse_from([&["linexusctl", "render-preview"], args].concat());
        assert!(parse(&["--out", "x.png"]).is_ok());
        assert!(parse(&["--template", "media", "--theme", "ember", "--out", "x.png"]).is_ok());
        assert!(parse(&["--config", "c.json", "--page", "p", "--out", "x.png"]).is_ok());
        assert!(parse(&[
            "--template",
            "media",
            "--config",
            "c.json",
            "--out",
            "x.png"
        ])
        .is_err());
        assert!(
            parse(&["--page", "p", "--out", "x.png"]).is_err(),
            "--page needs --config"
        );
        assert!(parse(&["--overlay", "notification", "--out", "x.png"]).is_ok());
        for other in [["--config", "c.json"], ["--pattern", "rgb-bars"]] {
            assert!(
                parse(&[&["--overlay", "volume", "--out", "x.png"][..], &other[..]].concat())
                    .is_err()
            );
        }
        assert!(parse(&[
            "--overlay",
            "track",
            "--template",
            "media",
            "--out",
            "x.png"
        ])
        .is_err());
        assert!(parse(&["--theme", "plaid", "--out", "x.png"]).is_err());
        assert!(parse(&["--pattern", "rgb-bars", "--scale", "0", "--out", "x.png"]).is_err());
    }

    #[test]
    fn push_joins_words() {
        let cli = Cli::try_parse_from([
            "linexusctl",
            "daemon",
            "push",
            "build",
            "done",
            "--icon",
            "check",
        ])
        .unwrap();
        let Command::Daemon {
            command: daemon_cmd::DaemonCommand::Push { text, icon, .. },
            ..
        } = cli.command
        else {
            panic!("expected daemon push");
        };
        assert_eq!(text.join(" "), "build done");
        assert_eq!(icon.as_deref(), Some("check"));
    }
}
