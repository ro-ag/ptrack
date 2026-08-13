use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("ptrack-db-import: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let command = raw
        .first()
        .and_then(|value| value.to_str())
        .filter(|value| matches!(*value, "import" | "activate" | "rollback"))
        .map(str::to_owned);
    if command.is_some() {
        raw.remove(0);
    }
    match command.as_deref().unwrap_or("import") {
        "import" => run_import(raw),
        "activate" => run_activate(raw),
        "rollback" => run_rollback(raw),
        _ => unreachable!(),
    }
}

fn run_import(raw: Vec<std::ffi::OsString>) -> Result<(), String> {
    let mut manifest = None;
    let mut destination = None;
    let mut accept_all = false;
    let mut arguments = raw.into_iter();
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
                    "Usage: ptrack-db-import import --manifest ABSOLUTE/manifest.json --destination ABSENT_ABSOLUTE_DIR --accept-all"
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

fn run_activate(raw: Vec<std::ffi::OsString>) -> Result<(), String> {
    let mut manifest = None;
    let mut candidates = None;
    let mut batch = None;
    let mut global_home = None;
    let mut generation = None;
    let mut writer_version = None;
    let mut accept_all = false;
    let mut arguments = raw.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--manifest") if manifest.is_none() => {
                manifest = Some(next_path(&mut arguments, "--manifest")?);
            }
            Some("--candidates") if candidates.is_none() => {
                candidates = Some(next_path(&mut arguments, "--candidates")?);
            }
            Some("--batch") if batch.is_none() => {
                batch = Some(next_path(&mut arguments, "--batch")?);
            }
            Some("--global-home") if global_home.is_none() => {
                global_home = Some(next_path(&mut arguments, "--global-home")?);
            }
            Some("--generation") if generation.is_none() => {
                generation = Some(
                    arguments
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or_else(|| "--generation requires one integer".to_owned())?
                        .parse::<u64>()
                        .map_err(|_| "--generation requires one integer".to_owned())?,
                );
            }
            Some("--writer-version") if writer_version.is_none() => {
                writer_version = Some(
                    arguments
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or_else(|| "--writer-version requires one value".to_owned())?,
                );
            }
            Some("--accept-all") if !accept_all => accept_all = true,
            Some("--help" | "-h") => {
                println!(
                    "Usage: ptrack-db-import activate --manifest ABSOLUTE/manifest.json --candidates ABSOLUTE/BATCH/candidates --batch ABSOLUTE/BATCH --global-home ABSOLUTE/PTRACK_HOME --generation NONZERO --writer-version VERSION --accept-all"
                );
                return Ok(());
            }
            _ => return Err("unknown or non-UTF-8 activation argument".to_owned()),
        }
    }
    let manifest = manifest.ok_or_else(|| "--manifest is required".to_owned())?;
    let candidates = candidates.ok_or_else(|| "--candidates is required".to_owned())?;
    let batch = batch.ok_or_else(|| "--batch is required".to_owned())?;
    let global_home = global_home.ok_or_else(|| "--global-home is required".to_owned())?;
    let writer_version = writer_version.ok_or_else(|| "--writer-version is required".to_owned())?;
    let receipt = ptrack_db_import::activate_stage(
        &manifest,
        &candidates,
        &batch,
        &global_home,
        generation.ok_or_else(|| "--generation is required".to_owned())?,
        &writer_version,
        accept_all,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "activated generation {}; {} destination(s); legacy sources retained; receipt={}/receipt.json",
        receipt.generation,
        receipt.destinations.len(),
        batch.display()
    );
    Ok(())
}

fn run_rollback(raw: Vec<std::ffi::OsString>) -> Result<(), String> {
    let mut batch = None;
    let mut global_home = None;
    let mut writer_version = None;
    let mut accept_all = false;
    let mut arguments = raw.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--batch") if batch.is_none() => {
                batch = Some(next_path(&mut arguments, "--batch")?);
            }
            Some("--global-home") if global_home.is_none() => {
                global_home = Some(next_path(&mut arguments, "--global-home")?);
            }
            Some("--writer-version") if writer_version.is_none() => {
                writer_version = Some(
                    arguments
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or_else(|| "--writer-version requires one value".to_owned())?,
                );
            }
            Some("--accept-all") if !accept_all => accept_all = true,
            Some("--help" | "-h") => {
                println!(
                    "Usage: ptrack-db-import rollback --batch ABSOLUTE/BATCH --global-home ABSOLUTE/PTRACK_HOME --writer-version VERSION --accept-all"
                );
                return Ok(());
            }
            _ => return Err("unknown or non-UTF-8 rollback argument".to_owned()),
        }
    }
    ptrack_db_import::rollback_activation(
        &batch.ok_or_else(|| "--batch is required".to_owned())?,
        &global_home.ok_or_else(|| "--global-home is required".to_owned())?,
        &writer_version.ok_or_else(|| "--writer-version is required".to_owned())?,
        accept_all,
    )
    .map_err(|error| error.to_string())?;
    println!("restored the previous active-generation marker; Rust databases retained");
    Ok(())
}

fn next_path(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires one path"))
}
