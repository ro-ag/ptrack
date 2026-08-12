use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("ptrack-db-import: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut manifest = None;
    let mut destination = None;
    let mut accept_all = false;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--manifest") if manifest.is_none() => {
                manifest = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--manifest requires one path".to_owned())?,
                ));
            }
            Some("--destination") if destination.is_none() => {
                destination =
                    Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        "--destination requires one path".to_owned()
                    })?));
            }
            Some("--accept-all") if !accept_all => accept_all = true,
            Some("--help" | "-h") => {
                println!(
                    "Usage: ptrack-db-import --manifest ABSOLUTE/manifest.json --destination ABSENT_ABSOLUTE_DIR --accept-all"
                );
                return Ok(());
            }
            _ => return Err("unknown or non-UTF-8 argument".to_owned()),
        }
    }
    let manifest = manifest.ok_or_else(|| "--manifest is required".to_owned())?;
    let destination = destination.ok_or_else(|| "--destination is required".to_owned())?;
    let receipt = ptrack_db_import::import_stage(&manifest, &destination, accept_all)
        .map_err(|error| error.to_string())?;
    println!(
        "created {} verified candidate(s); {} record(s), {} quarantined; receipt={}",
        receipt.candidate_count,
        receipt.report.record_count,
        receipt.report.quarantine_count,
        destination.join("receipt.json").display()
    );
    Ok(())
}
