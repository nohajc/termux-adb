use std::{
    env,
    process::{Command, ExitCode, ExitStatus},
    io, time::Duration, str,
    os::unix::process::CommandExt, path::Path,
};
use anyhow::{anyhow, Context};
use nix::{unistd, sys::signal};
use sysinfo::{SystemExt, RefreshKind, ProcessRefreshKind, ProcessExt, Pid};
use which::which;

fn get_termux_usb_list() -> Vec<String> {
    if let Ok(out) = Command::new("termux-usb").arg("-l").output() {
        if let Ok(stdout) = str::from_utf8(&out.stdout) {
            if let Ok(lst) = serde_json::from_str(stdout) {
                return lst;
            }
        }
    }
    vec![]
}

fn wait_for(pid: Pid) {
    let pid = unistd::Pid::from_raw(i32::from(pid));
    while let Ok(()) = signal::kill(pid, None) {
        std::thread::sleep(Duration::from_secs(1))
    };
}

fn run_under_termux_usb(usb_dev_path: &str, termux_adb_path: &Path) {
    Command::new("termux-usb")
        .env("TERMUX_USB_DEV", usb_dev_path)
        .arg("-e").arg(termux_adb_path)
        .args(["-E", "-r", usb_dev_path])
        .exec();
}

const REQUIRED_CMDS: [&str; 2] = ["adb", "termux-usb"];

fn check_dependencies() -> anyhow::Result<()> {
    for dep in REQUIRED_CMDS {
        _ = which(dep).context(format!("error: {} command not found", dep))?;
    }
    Ok(())
}

fn run_adb_kill_server() -> io::Result<ExitStatus> {
    Command::new("adb").arg("kill-server").status()
}

fn run_adb_start_server(termux_usb_dev: &str, termux_usb_fd: &str, adb_hooks_path: &Path) -> io::Result<ExitStatus> {
    Command::new("adb")
        .env("TERMUX_USB_DEV", termux_usb_dev)
        .env("TERMUX_USB_FD", termux_usb_fd)
        .env("LD_PRELOAD", adb_hooks_path)
        .arg("start-server")
        .status()
}

fn run() -> anyhow::Result<()> {
    check_dependencies()?;

    let termux_adb_path = env::current_exe()?;
    let adb_hooks_path = termux_adb_path.parent()
        .context("could not get directory of the executable")?
        .join("libadbhooks.so");

    match (env::var("TERMUX_USB_DEV"), env::var("TERMUX_USB_FD")) {
        (Ok(termux_usb_dev), Ok(termux_usb_fd)) => {
            println!("{}: fd = {}", &termux_usb_dev, termux_usb_fd);

            // 4. executes `adb kill-server && LD_PRELOAD=libadbhooks.so adb start-server`
            // (with TERMUX_USB_DEV and TERMUX_USB_FD env vars)
            let kill_status = run_adb_kill_server()?;
            if !kill_status.success() {
                return Err(anyhow!("adb kill-server exited with error status"));
            }

            let start_status = run_adb_start_server(&termux_usb_dev, &termux_usb_fd, &adb_hooks_path)?;
            if !start_status.success() {
                return Err(anyhow!("adb start-server exited with error status"));
            }

            let system = sysinfo::System::new_with_specifics(
                RefreshKind::new()
                    .with_processes(ProcessRefreshKind::new())
            );

            if let Some(p) = system.processes_by_exact_name("adb").next() {
                wait_for(p.pid());
            };
        }
        _ => {
            // 1. parses output of `termux-usb -l`
            let usb_dev_path = get_termux_usb_list()
                .into_iter().next().context("error: no usb device found")?;
            println!("using {}", &usb_dev_path);

            // 2. sets environment variable TERMUX_USB_DEV={usb_dev_path}
            // 3. executes termux-usb -e termux-adb -E -r {usb_dev_path}
            run_under_termux_usb(&usb_dev_path, &termux_adb_path);
        }
    }

    Ok(())
}

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("{}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
