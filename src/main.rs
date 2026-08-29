use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "__askpass")
    {
        return match rdp_tui::credentials::askpass::run_helper(&arguments[1..].join(" ")) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("rdp-tui askpass: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "__supervise")
    {
        return match rdp_tui::session::launcher::run_from_environment() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("rdp-tui supervise: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments.first().is_some_and(|argument| argument == "tui") {
        return match rdp_tui::tui::run(&config_root()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("rdp-tui: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match rdp_tui::cli::commands::run(&arguments, &config_root()) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rdp-tui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("rdp-tui")
}
