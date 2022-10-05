use std::{process::{Command, ExitCode, ExitStatus}, ffi::OsStr, io, time::Duration};
use nix::{unistd, sys::signal};
use sysinfo::{SystemExt, RefreshKind, ProcessRefreshKind, ProcessExt, Pid};

fn run_adb<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(args: I) -> io::Result<ExitStatus> {
    Command::new("adb").args(args).status()
}

fn wait_for(pid: Pid) {
    let pid = unistd::Pid::from_raw(i32::from(pid));
    while let Ok(()) = signal::kill(pid, None) {
        std::thread::sleep(Duration::from_secs(1))
    };
}

fn main() -> ExitCode {
    if let Err(e) = run_adb(["kill-server"]) {
        eprintln!("{}", e);
        return ExitCode::FAILURE;
    }

    if let Err(e) = run_adb(["start-server"]) {
        eprintln!("{}", e);
        return ExitCode::FAILURE;
    }

    let system = sysinfo::System::new_with_specifics(
        RefreshKind::new()
            .with_processes(ProcessRefreshKind::new())
    );

    if let Some(p) = system.processes_by_exact_name("adb").next() {
        wait_for(p.pid());
    }

    ExitCode::SUCCESS
}
