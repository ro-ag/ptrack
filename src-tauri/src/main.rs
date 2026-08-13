#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut application = ptrack_app::UnavailableApplication;
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let outcome = ptrack_cli::run(
        std::env::args_os(),
        &mut application,
        ptrack_cli::Io {
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
        },
    );
    match outcome {
        Ok(ptrack_cli::RunOutcome::ExitSuccess) => (),
        Ok(ptrack_cli::RunOutcome::LaunchGui { .. }) => run_desktop(),
        Ok(ptrack_cli::RunOutcome::LaunchTui) => {
            eprintln!("terminal UI is not implemented");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run_desktop() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run the p-track desktop shell");
}
