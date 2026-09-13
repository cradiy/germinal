#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalHyperlink {
    pub uri: String,
    pub x: u32,
    pub y: u32,
    pub columns: u32,
}

impl TerminalHyperlink {
    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.y == y && x >= self.x && x < self.x.saturating_add(self.columns)
    }
}

pub fn is_supported_terminal_hyperlink_uri(uri: &str) -> bool {
    if uri.is_empty() || uri.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, remainder)) = uri.split_once(':') else {
        return false;
    };
    if remainder.is_empty() {
        return false;
    }

    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "file"
            | "ftp"
            | "ftps"
            | "gemini"
            | "git"
            | "gopher"
            | "http"
            | "https"
            | "irc"
            | "ircs"
            | "mailto"
            | "news"
            | "sftp"
            | "ssh"
    )
}

#[cfg(test)]
mod tests {
    use super::is_supported_terminal_hyperlink_uri;

    #[test]
    fn accepts_common_terminal_uri_schemes_case_insensitively() {
        for uri in [
            "https://example.com/docs",
            "HTTP://example.com",
            "file:///tmp/readme.txt",
            "ftp://example.com/archive",
            "mailto:user@example.com",
            "ssh://host.example.com",
        ] {
            assert!(is_supported_terminal_hyperlink_uri(uri), "{uri}");
        }
    }

    #[test]
    fn rejects_bare_text_unsafe_schemes_and_control_characters() {
        for uri in [
            "example.com",
            "https:",
            "javascript:alert(1)",
            "https://example.com\ncommand",
        ] {
            assert!(!is_supported_terminal_hyperlink_uri(uri), "{uri:?}");
        }
    }
}
