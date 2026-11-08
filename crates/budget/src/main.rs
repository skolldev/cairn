//! `cairn-budget`: run through `scripts/budget.sh`. See `docs/budget-harness.md`.

use std::path::PathBuf;

use cairn_budget::report::{self, Row};
use cairn_budget::run::{self, Config};

const USAGE: &str = "\
usage: cairn-budget run [--only launch,write,external] [options]
       cairn-budget held-key [options]

options:
  --cairn PATH       the cairn binary, built with --features measure
                     (default: next to this one)
  --presentmon PATH  PresentMon's console exe, on Windows (default: $CAIRN_PRESENTMON)
  --runs N           runs per Budget (default 3; fewer can only come out Invalid)
  --samples N        samples per run (default 20; fewer can only come out Invalid)
  --out DIR          where the result goes (default budget-results)";

struct Args {
    command: String,
    only: Vec<String>,
    out: PathBuf,
    config: Config,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(USAGE)?;
    let binary = if cfg!(windows) { "cairn.exe" } else { "cairn" };
    let mut parsed = Args {
        command,
        only: vec![
            "launch".to_owned(),
            "write".to_owned(),
            "external".to_owned(),
        ],
        out: PathBuf::from("budget-results"),
        config: Config {
            cairn: std::env::current_exe()
                .map_err(|e| e.to_string())?
                .with_file_name(binary),
            presentmon: std::env::var_os("CAIRN_PRESENTMON").map(PathBuf::from),
            fixtures: std::env::var_os("CAIRN_FIXTURES")
                .map(PathBuf::from)
                .or_else(|| {
                    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
                    std::env::var_os(home).map(|h| PathBuf::from(h).join("cairn-fixtures"))
                })
                .ok_or("neither CAIRN_FIXTURES nor a home directory is set")?,
            runs: 3,
            samples: 20,
        },
    };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value\n{USAGE}"));
        match flag.as_str() {
            "--only" => parsed.only = value()?.split(',').map(str::to_owned).collect(),
            "--cairn" => parsed.config.cairn = value()?.into(),
            "--presentmon" => parsed.config.presentmon = Some(value()?.into()),
            "--runs" => {
                parsed.config.runs = value()?.parse().map_err(|e| format!("--runs: {e}"))?
            }
            "--samples" => {
                parsed.config.samples = value()?.parse().map_err(|e| format!("--samples: {e}"))?
            }
            "--out" => parsed.out = value()?.into(),
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(parsed)
}

/// Points git at an empty config, for the harness and the Cairn it starts, so that a machine's
/// `core.fsmonitor` or hooks can't change a number. Before anything spawns git.
fn hermetic() -> Result<tempfile::TempDir, String> {
    let home = tempfile::tempdir().map_err(|e| e.to_string())?;
    // SAFETY: nothing else is running yet: `main` calls this before any thread or git starts.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    Ok(home)
}

fn measure(args: &Args) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    match args.command.as_str() {
        "held-key" => rows.push(run::measure_held_key(&args.config)?),
        "run" => {
            for name in &args.only {
                match name.as_str() {
                    "launch" => rows.push(run::measure_launch(&args.config)?),
                    "write" => rows.push(run::measure_local_write(&args.config)?),
                    "external" => rows.extend(run::measure_external_change(&args.config)?),
                    _ => return Err(format!("no Budget named {name}\n{USAGE}")),
                }
            }
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(rows)
}

fn run() -> Result<i32, String> {
    if std::env::var_os("CI").is_some() {
        return Err("this is a CI runner, and Budget numbers never count from one".to_owned());
    }
    let args = parse_args()?;
    let _home = hermetic()?;
    let repo = std::env::current_dir().map_err(|e| e.to_string())?;
    let environment = run::environment(&args.config, &repo)?;
    let rows = measure(&args)?;

    let text = report::render(&environment, &rows);
    std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let path = args.out.join(format!("{}-{stamp}.md", args.command));
    std::fs::write(&path, &text).map_err(|e| e.to_string())?;
    println!("{text}");
    eprintln!("Wrote {}", path.display());
    Ok(report::verdict_exit_code(&rows))
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    }
}
