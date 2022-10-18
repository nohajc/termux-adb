use std::{
    env,
    process::{Command, ExitCode, ExitStatus},
    io::{self, BufRead}, time::Duration, str,
    os::{
        unix::{
            net::UnixDatagram,
            process::CommandExt,
        },
        raw::c_int
    },
    path::{Path, PathBuf}, thread, fs::File,
};
use anyhow::{anyhow, Context};
use daemonize::Daemonize;
use nix::{unistd, sys::signal::{self, Signal}};
use sysinfo::{SystemExt, RefreshKind, ProcessRefreshKind, ProcessExt, Pid};
use which::which;

use crossbeam_channel::{bounded, tick, Receiver, select};

use signal_hook::{
    consts::signal::*,
    iterator::Signals,
};

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

fn new_signal_receiver() -> anyhow::Result<Receiver<c_int>> {
    let mut signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT])?;

    let (sender, receiver) = bounded(128);
    thread::spawn(move || {
        for sig in signals.forever() {
            sender.send(sig).expect("error processing signal");
        }
    });

    Ok(receiver)
}

fn wait_for_adb_start(log_file_path: PathBuf) -> anyhow::Result<()> {
    let log_file = File::open(&log_file_path).with_context(
        || format!("could not open log file {} for reading", log_file_path.display())
    )?;

    let mut log_file_lines = io::BufReader::new(log_file).lines();

    let mut i = 0;
    let mut waiting_for_device = true;
    while i < 20 { // after the device is approved, wait 5 secs for adb to start
        while let Some(msg) = log_file_lines.next().map(
            |ln| ln.ok()
        ).flatten() {
            if msg.contains("adbhooks]") {
                continue;
            }
            println!("{}", msg);
            if msg == "* daemon started successfully" {
                return Ok(());
            }
            if msg.starts_with("using /dev/bus/usb") || msg == "no device connected yet" {
                waiting_for_device = false;
            }
        }
        if !waiting_for_device {
            i += 1;
        }
        thread::sleep(Duration::from_millis(250));
    }

    let log_file_str = log_file_path.to_string_lossy();
    let adb_log = log_file_str.replacen("termux-adb.", "adb.", 1);
    Err(anyhow!("error: adb server didn't start, check the log: {}", adb_log))
}

fn scan_for_usb_devices() -> anyhow::Result<()> {
    let sock_path = "/data/data/com.termux/files/usr/tmp/termux-adb.sock";
    let socket = UnixDatagram::unbound().context("failed to create UDS")?;
    let mut socket_connected = false;

    loop {
        if !socket_connected {
            match socket.connect(sock_path) {
                Ok(()) => {
                    socket_connected = true;
                    println!("socket connected");
                }
                Err(e) => {
                    eprintln!("socket connect error: {}", e);
                }
            }
        }
        if socket_connected {
            println!("| sending message to socket");
            match socket.send(b"hello from termux-adb monitor") {
                Ok(size) => println!("> message sent to socket: size={}", size),
                Err(e) => eprintln!("socket send error: {}", e)
            }
        }
        thread::sleep(Duration::from_millis(2500));
    }
}

fn wait_for_adb_end(pid: Pid, signals: Receiver<c_int>) {
    let pid = unistd::Pid::from_raw(i32::from(pid));
    let ticker = tick(Duration::from_secs(1));

    let mut kill_signal = None;
    let mut kill_cnt = 0;
    loop {
        select! {
            recv(ticker) -> _ => {
                if let Some(_) = kill_signal {
                    kill_cnt += 1;
                    if kill_cnt > 3 {
                        kill_signal = Some(Signal::SIGKILL);
                    }
                }

                if let Err(_) = signal::kill(pid, kill_signal) {
                    break;
                }
            }
            recv(signals) -> _ => {
                // we received a termination request
                // so instead of checking if adb is alive
                // we'll switch to actively trying to kill it
                kill_signal = Some(Signal::SIGTERM);
                if let Err(_) = signal::kill(pid, kill_signal) {
                    break;
                }
            }
        }
    }
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

fn get_tmp_dir_path() -> PathBuf {
    PathBuf::from(env::var("TMPDIR").unwrap_or("/tmp".to_owned()))
}

fn get_log_file_path(base_dir: &Path) -> PathBuf {
    let mut result = base_dir.to_owned();
    result.push(format!("termux-adb.{}.log", unistd::getuid()));
    result
}

fn phase_two(termux_usb_dev: &str, termux_usb_fd: &str, adb_hooks_path: &Path) -> anyhow::Result<()> {
    println!("using {}: fd = {}", &termux_usb_dev, termux_usb_fd);

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

    // 5. attach signal handler which kills adb before termux-adb is terminated itself
    // 6. finds adb server PID and waits for it
    if let Some(p) = system.processes_by_exact_name("adb").next() {
        thread::spawn(|| {
            if let Err(e) = scan_for_usb_devices() {
                eprintln!("{}", e);
            }
        });
        wait_for_adb_end(p.pid(), new_signal_receiver()?);
    };

    Ok(())
}

// TODO: termux-adb could continually keep track of usb devices and send valid
// file descriptors to libadbhooks.so using some IPC mechanism like unix domain socket
//
// for that to work though, it has to keep execing itself through termux-usb
// but not restart adb server each time; that means we want to check if libadbhooks.so
// is already injected and that can be determined by reading from procfs memory map
// (https://docs.rs/procfs/latest/procfs/process/struct.Process.html#method.maps)
fn run() -> anyhow::Result<()> {
    check_dependencies()?;

    let termux_adb_path = env::current_exe()?;
    let adb_hooks_path = termux_adb_path.parent()
        .context("could not get directory of the executable")?
        .join("libadbhooks.so");

    if !adb_hooks_path.exists() {
        return Err(anyhow!("error: could not find libadbhooks.so"))
    }

    match (env::var("TERMUX_USB_DEV"), env::var("TERMUX_USB_FD")) {
        (Ok(termux_usb_dev), Ok(termux_usb_fd)) => {
            phase_two(&termux_usb_dev, &termux_usb_fd, &adb_hooks_path)?;
        }
        _ => {
            let tmpdir = get_tmp_dir_path();
            let log_file_path = get_log_file_path(&tmpdir);
            let log_file_stdout = File::create(&log_file_path).with_context(
                || format!("could not create log file in {}", tmpdir.display())
            )?;
            let log_file_stderr = log_file_stdout.try_clone()?;

            let daemonize = Daemonize::new()
                .working_directory(&tmpdir)
                .stdout(log_file_stdout)
                .stderr(log_file_stderr)
                .exit_action(|| {
                    if let Err(e) = wait_for_adb_start(log_file_path) {
                        eprintln!("{}", e);
                    }
                });

            // 1. parses output of `termux-usb -l`
            let usb_dev_path = get_termux_usb_list().into_iter()
                .next().map(|p| { println!("requesting {}", &p); p });

            daemonize.start()?; // everything below runs in the background

            if let Some(usb_dev_path) = usb_dev_path {
                // 2. sets environment variable TERMUX_USB_DEV={usb_dev_path}
                // 3. executes termux-usb -e termux-adb -E -r {usb_dev_path}
                run_under_termux_usb(&usb_dev_path, &termux_adb_path);
            } else {
                println!("no device connected yet");
                // if no usb device found yet, start adb server directly
                phase_two("none", "-1", &adb_hooks_path)?;
            }
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
