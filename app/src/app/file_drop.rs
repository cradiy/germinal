use std::path::Path;

/// Quote one dropped path as an argument, never as a command to execute.
pub(super) fn path_argument(path: &Path, program: &str) -> Result<String, &'static str> {
    let path = path.to_str().ok_or("file path is not valid Unicode")?;
    if path.chars().any(char::is_control) {
        return Err("file path contains terminal control characters");
    }
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    let quoted = match name {
        "powershell" | "pwsh" => {
            // PowerShell treats typographic apostrophes as quote delimiters too.
            let mut escaped = String::new();
            for ch in path.chars() {
                escaped.push(ch);
                if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                    escaped.push(ch);
                }
            }
            format!("'{escaped}'")
        }
        "cmd" => {
            // Quoting does not suppress cmd's percent/delayed expansion.
            if path.contains(['%', '!', '"']) {
                return Err("cmd cannot safely quote this path with unknown expansion settings");
            }
            format!("\"{path}\"")
        }
        "nu" => {
            let mut hashes = String::from("#");
            while path.contains(&format!("'{hashes}")) {
                hashes.push('#');
            }
            format!("r{hashes}'{path}'{hashes}")
        }
        "bash" | "sh" | "zsh" | "dash" | "niu" | "niubash" => {
            format!("'{}'", path.replace('\'', "'\"'\"'"))
        }
        "fish" => format!("'{}'", path.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => return Err("file drop quoting is unavailable for this shell"),
    };
    // Separate successive DroppedFile events and any command already being typed.
    Ok(format!(" {quoted} "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_remain_literal_shell_arguments() {
        let path = Path::new("C:\\项目 A\\O'Brien$demo.txt");
        assert_eq!(
            path_argument(path, "C:\\Windows\\pwsh.exe").unwrap(),
            " 'C:\\项目 A\\O''Brien$demo.txt' "
        );
        assert_eq!(
            path_argument(path, "niu.exe").unwrap(),
            " 'C:\\项目 A\\O'\"'\"'Brien$demo.txt' "
        );
        assert_eq!(
            path_argument(Path::new("C:\\a'#b"), "nu").unwrap(),
            " r##'C:\\a'#b'## "
        );
        assert_eq!(
            path_argument(Path::new("C:\\A & B\\file.txt"), "cmd.exe").unwrap(),
            " \"C:\\A & B\\file.txt\" "
        );
        assert!(path_argument(Path::new("C:\\%PATH%.txt"), "cmd.exe").is_err());
        for program in ["pwsh", "cmd", "nu", "bash"] {
            assert!(path_argument(Path::new("bad\ncommand"), program).is_err());
            assert!(path_argument(Path::new("bad\u{1b}[31m"), program).is_err());
        }
    }
}
