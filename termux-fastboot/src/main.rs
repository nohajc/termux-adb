use anyhow::Context;
use std::{
    env, iter, os::unix::process::CommandExt,
    str, process::{Command, ExitCode},
    ffi::{OsString, OsStr},
};
use which::which;

const REQUIRED_CMDS: [&str; 2] = ["fastboot", "termux-usb"];

fn check_dependencies() -> anyhow::Result<()> {
    for dep in REQUIRED_CMDS {
        _ = which(dep).context(format!("error: {} command not found", dep))?;
    }
    Ok(())
}

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

fn run_fastboot_under_termux_usb(usb_dev_path: &str) {
    let mut cmd = Command::new("termux-usb");

    let fastboot = iter::once(OsString::from("fastboot"))
        .chain(env::args_os().skip(1))
        .collect::<Vec<OsString>>().join(OsStr::new(" "));

    cmd.arg("-e").arg(fastboot)
        .args(["-E", "-r", usb_dev_path]);

    cmd.exec();
}

fn run() -> anyhow::Result<()> {
    check_dependencies()?;

    if let Some(usb_dev_path) = get_termux_usb_list().into_iter().next() {
        run_fastboot_under_termux_usb(&usb_dev_path);
    } else {
        println!("no USB device found");
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
