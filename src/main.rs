use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    // FreeRDP invokes $FREERDP_ASKPASS with the prompt as argv (not "__askpass"),
    // so askpass mode is recognized by the inherited descriptor environment the
    // launcher set. The explicit "__askpass" argument stays supported for tests.
    let explicit_askpass = arguments
        .first()
        .is_some_and(|argument| argument == "__askpass");
    let inherited_askpass = std::env::var_os("RDP_TUI_ASKPASS_MAIN_FD").is_some()
        || std::env::var_os("RDP_TUI_ASKPASS_GATEWAY_FD").is_some();
    if explicit_askpass || inherited_askpass {
        let prompt = if explicit_askpass {
            arguments[1..].join(" ")
        } else {
            arguments.join(" ")
        };
        return match rdp_tui::credentials::askpass::run_helper(&prompt) {
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
