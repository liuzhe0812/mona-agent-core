//! Child-only native launcher. Invoke before Tokio, threads, credentials or product setup.
use crate::{Error, Mode, Policy, Result};
use std::{ffi::OsString, path::PathBuf};

/// Returns None for an ordinary host invocation. Helper errors never start the host.
pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.as_os_str() != "--mona-sandbox" {
        return None;
    }
    Some(run(args.collect()))
}

pub fn run(args: Vec<OsString>) -> i32 {
    let backend = args.first().and_then(|s| s.to_str()).unwrap_or("");
    let (signature, exit) = if backend == "landlock" {
        ("mona-landlock-run", 125)
    } else {
        ("mona-windows-acl-run", 127)
    };
    match execute(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{signature}: {error}");
            exit
        }
    }
}
fn execute(args: &[OsString]) -> Result<i32> {
    #[cfg(target_os = "linux")]
    if args.len() == 2 && args[0] == "landlock" && args[1] == "--probe" {
        let status = restrict_landlock(&Policy::new(Mode::ReadOnly, "/")?)?;
        return Ok(if status { 0 } else { 10 });
    }
    let mut mode = None;
    let mut workspace = None;
    let mut temp = None;
    let mut i = 1;
    while i < args.len() && args[i] != "--" {
        let key = args[i]
            .to_str()
            .ok_or_else(|| Error::config("non-text runner flag"))?;
        let value = args
            .get(i + 1)
            .ok_or_else(|| Error::config("missing runner flag value"))?;
        match key {
            "--mode" if mode.is_none() => {
                mode = Some(
                    value
                        .to_str()
                        .ok_or_else(|| Error::config("non-text sandbox mode"))?
                        .parse::<Mode>()?,
                )
            }
            "--workspace" if workspace.is_none() => workspace = Some(PathBuf::from(value)),
            "--temp" if temp.is_none() => temp = Some(PathBuf::from(value)),
            _ => return Err(Error::config("unknown or repeated sandbox runner flag")),
        }
        i += 2;
    }
    if args.get(i).is_none_or(|s| s != "--") || i + 1 >= args.len() {
        return Err(Error::config("runner requires -- and a command"));
    }
    let policy = Policy::new(
        mode.ok_or_else(|| Error::config("missing sandbox mode"))?,
        workspace.ok_or_else(|| Error::config("missing workspace"))?,
    )?;
    if !policy.mode.confined() {
        return Err(Error::config(
            "native confinement runner does not accept unrestricted mode",
        ));
    }
    let argv = &args[i + 1..];
    match args.first().and_then(|s| s.to_str()) {
        #[cfg(windows)]
        Some("windows-acl") => crate::windows::run(&policy, temp.as_deref(), argv),
        #[cfg(target_os = "linux")]
        Some("landlock") => {
            if temp.is_some() {
                return Err(Error::config("Landlock does not accept a temp override"));
            }
            if !restrict_landlock(&policy)? {
                eprintln!("mona-landlock-run: partial enforcement (older Landlock ABI)");
            }
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
            Err(Error::unavailable(format!("exec failed: {error}")))
        }
        _ => {
            let _ = (policy, temp, argv);
            Err(Error::unavailable(
                "runner backend does not match this platform",
            ))
        }
    }
}

#[cfg(target_os = "linux")]
fn restrict_landlock(policy: &Policy) -> Result<bool> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };
    // ABI V5 is the complete file-effect vocabulary used here, including truncate/ioctl.
    // Newer network/scoped restrictions are intentionally not requested.
    let abi = ABI::V5;
    let handled = AccessFs::from_all(abi);
    let read = AccessFs::from_read(abi);
    let write = handled & !read;
    let mut rules = Ruleset::default()
        .handle_access(handled)
        .map_err(Error::unavailable)?
        .create()
        .map_err(Error::unavailable)?;
    rules = rules
        .add_rule(PathBeneath::new(
            PathFd::new("/").map_err(Error::unavailable)?,
            read,
        ))
        .map_err(Error::unavailable)?;
    // File grants must not carry directory-only rights. Each mandatory path is opened explicitly.
    rules = rules
        .add_rule(PathBeneath::new(
            PathFd::new("/dev/null").map_err(Error::unavailable)?,
            AccessFs::WriteFile | AccessFs::Truncate | AccessFs::IoctlDev,
        ))
        .map_err(Error::unavailable)?;
    if policy.mode == Mode::WorkspaceWrite {
        for root in [
            std::path::Path::new("/tmp"),
            policy.workspace_root.as_path(),
        ] {
            rules = rules
                .add_rule(PathBeneath::new(
                    PathFd::new(root).map_err(Error::unavailable)?,
                    write,
                ))
                .map_err(Error::unavailable)?;
        }
    }
    let status = rules.restrict_self().map_err(Error::unavailable)?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(true),
        RulesetStatus::PartiallyEnforced => Ok(false),
        _ => Err(Error::unavailable(
            "kernel did not confirm Landlock enforcement",
        )),
    }
}
