use std::env;
use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use ptrack_migrate::validate_path;

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("ptrack-migrate: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[std::ffi::OsString]) -> Result<(), String> {
    if arguments.len() == 3 && arguments[0] == "inspect" && arguments[1] == "--bundle" {
        return inspect(Path::new(&arguments[2]));
    }
    Err("usage: ptrack-migrate inspect --bundle ABSOLUTE_PATH".to_owned())
}

fn inspect(path: &Path) -> Result<(), String> {
    let bundle = validate_path(path).map_err(|error| error.to_string())?;
    let digest = bundle
        .sha256()
        .iter()
        .fold(String::new(), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    println!(
        "valid {} bundle: source_format={}, buckets={}, records={}, bytes={}, sha256={digest}",
        bundle.kind(),
        bundle.source_format(),
        bundle.buckets().len(),
        bundle.total_records(),
        bundle.byte_len(),
    );
    Ok(())
}
