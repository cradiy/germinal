//! Shared Windows shell resolution for configuration validation and ConPTY.
use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use portable_pty::CommandBuilder;

const BATCH_COMMAND_ENV: &str = "GERMINAL_INTERNAL_BATCH_COMMAND";

pub fn command(
    program: &str,
    args: &[String],
    working_directory: Option<&Path>,
) -> Result<CommandBuilder, String> {
    let cwd = working_directory
        .map(Path::to_path_buf)
        .or_else(|| env::current_dir().ok())
        .ok_or("cannot determine shell working directory")?;
    let path = env::var_os("PATH").unwrap_or_default();
    let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let program = resolve(program, &cwd, &path, &extensions)
        .ok_or_else(|| format!("shell program was not found: {program}"))?;
    let extension = program
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("bat") {
        let mut command = CommandBuilder::new(&program);
        command.args(args);
        configure_directory_reporting(&mut command, &program, args);
        return Ok(command);
    }

    // portable-pty applies CRT argument quoting, which is not cmd.exe quoting.
    // Expand one environment token instead: expansion is not recursive, so literal
    // percent signs survive. Quote every value and disable delayed (!) expansion.
    let mut line = quote_batch_value(&program.to_string_lossy().replace('/', "\\"))?;
    for arg in args {
        line.push(' ');
        line.push_str(&quote_batch_value(arg)?);
    }
    let interpreter = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("SystemRoot is required to locate cmd.exe")?
        .join("System32")
        .join("cmd.exe");
    let mut command = CommandBuilder::new(interpreter);
    command.args([
        "/d",
        "/v:off",
        "/s",
        "/c",
        "%GERMINAL_INTERNAL_BATCH_COMMAND%",
    ]);
    command.env(BATCH_COMMAND_ENV, line);
    Ok(command)
}

const POWERSHELL_DIRECTORY_REPORTING: &str = include_str!("powershell_directory_reporting.ps1");

fn configure_directory_reporting(command: &mut CommandBuilder, program: &Path, args: &[String]) {
    let name = program
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("cmd") && args.is_empty() {
        let prompt = env::var("PROMPT").unwrap_or_else(|_| "$P$G".into());
        command.env("PROMPT", format!("$E]9;9;$P$E\\{prompt}"));
    } else if (name.eq_ignore_ascii_case("powershell") || name.eq_ignore_ascii_case("pwsh"))
        // Only augment known interactive switches; leave command/script modes and
        // unfamiliar options intact instead of changing their execution semantics.
        && args.iter().all(|arg| ["-nologo", "-noprofile", "-noexit"].iter().any(|allowed| arg.eq_ignore_ascii_case(allowed)))
    {
        use base64::Engine;
        let script: Vec<u8> = POWERSHELL_DIRECTORY_REPORTING
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        if !args.iter().any(|arg| arg.eq_ignore_ascii_case("-noexit")) {
            command.arg("-NoExit");
        }
        command.arg("-EncodedCommand");
        command.arg(base64::engine::general_purpose::STANDARD.encode(script));
    }
}

fn quote_batch_value(value: &str) -> Result<String, String> {
    if value.contains(['"', '\r', '\n', '\0']) {
        return Err(
            "batch shell paths and arguments cannot contain quotes, newlines, or NUL".into(),
        );
    }
    Ok(format!("\"{value}\""))
}

pub(super) fn resolve(
    program: &str,
    cwd: &Path,
    search_path: &OsStr,
    extensions: &str,
) -> Option<PathBuf> {
    let path = Path::new(program);
    let directories = if path.is_absolute() || path.components().count() > 1 {
        vec![cwd.to_path_buf()]
    } else {
        env::split_paths(search_path)
            .map(|directory| cwd.join(directory))
            .collect()
    };
    for directory in directories {
        let candidate = directory.join(path);
        if candidate.is_file() {
            return std::path::absolute(candidate).ok();
        }
        if path.extension().is_some() {
            continue;
        }
        for extension in extensions.split(';').map(str::trim) {
            // File associations such as .PS1 are not directly launchable by ConPTY.
            if ![".exe", ".com", ".cmd", ".bat"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
            {
                continue;
            }
            let mut name = candidate.as_os_str().to_os_string();
            name.push(extension);
            let candidate = PathBuf::from(name);
            if candidate.is_file() {
                return std::path::absolute(candidate).ok();
            }
        }
    }
    None
}
