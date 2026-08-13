#[cfg(unix)]
use std::io;

#[cfg(any(unix, windows))]
use crate::{NativePtyFactory, PtyFactory, StartRequest};

#[cfg(unix)]
#[test]
fn native_pty_is_interactive_unicode_resizable_and_preserves_exit_code() {
    let environment = std::env::vars_os()
        .filter_map(|(key, value)| {
            Some(format!(
                "{}={}",
                key.into_string().ok()?,
                value.into_string().ok()?
            ))
        })
        .collect();
    let process = NativePtyFactory
        .start(StartRequest {
            executable: "/bin/sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf 'ready:'; read line; printf '%s' \"$line\"; exit 7".to_owned(),
            ],
            env: environment,
            cwd: "/tmp".into(),
            rows: 24,
            columns: 80,
        })
        .unwrap();
    process.resize(37, 119).unwrap();
    let input = "héllo-世界\n".as_bytes();
    let mut remaining = input;
    while !remaining.is_empty() {
        let written = process.write(remaining).unwrap();
        assert_ne!(written, 0);
        remaining = &remaining[written..];
    }
    let mut output = Vec::new();
    let mut buffer = [0_u8; 256];
    loop {
        match process.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => panic!("read native PTY: {error}"),
        }
    }
    assert!(String::from_utf8_lossy(&output).contains("héllo-世界"));
    assert_eq!(process.wait().unwrap(), 7);
    process.close().unwrap();
}

#[cfg(unix)]
#[test]
fn native_pty_normalizes_eio_as_eof() {
    let error = Err(io::Error::from_raw_os_error(
        rustix::io::Errno::IO.raw_os_error(),
    ));
    assert_eq!(super::pty::normalize_pty_read(error).unwrap(), 0);
}

#[test]
fn windows_pty_environment_parser_preserves_drive_directory_entries() {
    assert_eq!(
        super::pty::split_windows_environment_entry("=C:=C:\\work").unwrap(),
        ("=C:", "C:\\work")
    );
    assert_eq!(
        super::pty::split_windows_environment_entry("Path=C:\\Windows").unwrap(),
        ("Path", "C:\\Windows")
    );
    for invalid in ["", "NO-SEPARATOR", "=C:"] {
        assert!(super::pty::split_windows_environment_entry(invalid).is_err());
    }
}

#[cfg(windows)]
#[test]
fn windows_force_close_interrupts_wait_and_kills_the_descendant_job() {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let pid_file = std::env::temp_dir().join(format!(
        "ptrack-conpty-descendant-{}-{}.pid",
        std::process::id(),
        getrandom::u64().unwrap()
    ));
    let escaped = pid_file.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$p=Start-Process powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 120' -PassThru; Set-Content -LiteralPath '{escaped}' -Value $p.Id; Wait-Process -Id $p.Id"
    );
    let environment = std::env::vars_os()
        .filter_map(|(key, value)| {
            Some(format!(
                "{}={}",
                key.into_string().ok()?,
                value.into_string().ok()?
            ))
        })
        .collect();
    let process: Arc<dyn super::PtyProcess> = Arc::from(
        NativePtyFactory
            .start(StartRequest {
                executable: "powershell.exe".to_owned(),
                args: vec!["-NoProfile".to_owned(), "-Command".to_owned(), script],
                env: environment,
                cwd: std::env::temp_dir(),
                rows: 24,
                columns: 80,
            })
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let descendant_pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let (finished_tx, finished_rx) = mpsc::channel();
    let waiter = Arc::clone(&process);
    let wait_thread = std::thread::spawn(move || finished_tx.send(waiter.wait()).unwrap());

    process.kill().unwrap();

    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Job kill must interrupt the wait mutex")
        .unwrap();
    wait_thread.join().unwrap();
    process.close().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists_windows(descendant_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_exists_windows(descendant_pid));
    let _ = std::fs::remove_file(pid_file);
}

#[cfg(windows)]
fn process_exists_windows(pid: u32) -> bool {
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
            ),
        ])
        .status()
        .is_ok_and(|status| status.success())
}
