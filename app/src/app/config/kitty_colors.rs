use std::{fs, path::Path};

use germinal_ports::{
    pty_host::color_theme::TerminalColorTheme, rendering::frame_plan_builder::RgbColorDto,
};

use super::{ColorsConfig, expand_home};

#[derive(Debug)]
struct ThemeEntry {
    key: String,
    value: String,
    origin: String,
    strict_key: bool,
}

pub(super) fn resolve_color_theme(
    config: &ColorsConfig,
    config_dir: &Path,
) -> Result<TerminalColorTheme, String> {
    let mut entries = Vec::new();
    if let Some(theme_path) = config.theme.as_deref() {
        let theme_path = expand_home(theme_path);
        let theme_path = if theme_path.is_absolute() {
            theme_path
        } else {
            config_dir.join(theme_path)
        };
        let contents = fs::read_to_string(&theme_path).map_err(|error| {
            format!(
                "failed to read Kitty color theme {}: {error}",
                theme_path.display()
            )
        })?;
        entries.extend(parse_kitty_theme(
            &contents,
            &theme_path.display().to_string(),
        ));
    }
    entries.extend(config.overrides.iter().map(|(key, value)| ThemeEntry {
        key: key.clone(),
        value: value.clone(),
        origin: format!("[colors].{key}"),
        strict_key: true,
    }));

    if entries.is_empty() {
        return Ok(TerminalColorTheme::default());
    }

    let mut theme = TerminalColorTheme::default();
    for entry in &entries {
        match entry.key.as_str() {
            "foreground" => theme.foreground = parse_entry_rgb(entry)?,
            "background" => theme.background = parse_entry_rgb(entry)?,
            key if palette_index(key).is_some() => {
                theme.palette[palette_index(key).expect("palette key checked")] =
                    parse_entry_rgb(entry)?;
            }
            key if !is_supported_key(key) && entry.strict_key => {
                return Err(format!("unsupported Kitty color key: {}", entry.origin));
            }
            _ => {}
        }
    }

    theme.cursor = theme.foreground;
    theme.cursor_text = theme.background;
    theme = theme.with_derived_host_colors();

    for entry in &entries {
        match entry.key.as_str() {
            "cursor" => {
                theme.cursor = parse_dynamic_rgb(entry, theme.foreground, &["none"])?;
            }
            "cursor_text_color" => {
                theme.cursor_text =
                    parse_dynamic_rgb(entry, theme.background, &["background", "none"])?;
            }
            "selection_foreground" => {
                theme.selection_foreground = parse_optional_rgb(entry)?;
            }
            "selection_background" => {
                theme.selection_background = parse_optional_rgb(entry)?;
            }
            "url_color" => theme.url = parse_entry_rgb(entry)?,
            "active_border_color" => theme.active_border = parse_entry_rgb(entry)?,
            "inactive_border_color" => theme.inactive_border = parse_entry_rgb(entry)?,
            "bell_border_color" => theme.bell_border = parse_entry_rgb(entry)?,
            "active_tab_foreground" => theme.active_tab_foreground = parse_entry_rgb(entry)?,
            "active_tab_background" => theme.active_tab_background = parse_entry_rgb(entry)?,
            "inactive_tab_foreground" => theme.inactive_tab_foreground = parse_entry_rgb(entry)?,
            "inactive_tab_background" => theme.inactive_tab_background = parse_entry_rgb(entry)?,
            _ => {}
        }
    }

    Ok(theme)
}

fn parse_kitty_theme(contents: &str, source: &str) -> Vec<ThemeEntry> {
    contents
        .lines()
        .enumerate()
        .filter_map(|(line_index, line)| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut fields = line.split_whitespace();
            let key = fields.next()?;
            let value = fields.next()?;
            is_supported_key(key).then(|| ThemeEntry {
                key: key.to_string(),
                value: trim_quotes(value).to_string(),
                origin: format!("{source}:{}", line_index + 1),
                strict_key: false,
            })
        })
        .collect()
}

fn is_supported_key(key: &str) -> bool {
    matches!(
        key,
        "foreground"
            | "background"
            | "cursor"
            | "cursor_text_color"
            | "selection_foreground"
            | "selection_background"
            | "url_color"
            | "active_border_color"
            | "inactive_border_color"
            | "bell_border_color"
            | "active_tab_foreground"
            | "active_tab_background"
            | "inactive_tab_foreground"
            | "inactive_tab_background"
    ) || palette_index(key).is_some()
}

fn palette_index(key: &str) -> Option<usize> {
    key.strip_prefix("color")?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < 256)
}

fn parse_entry_rgb(entry: &ThemeEntry) -> Result<RgbColorDto, String> {
    parse_rgb(&entry.value)
        .ok_or_else(|| format!("invalid Kitty color '{}' at {}", entry.value, entry.origin))
}

fn parse_dynamic_rgb(
    entry: &ThemeEntry,
    fallback: RgbColorDto,
    dynamic_values: &[&str],
) -> Result<RgbColorDto, String> {
    if dynamic_values
        .iter()
        .any(|value| entry.value.eq_ignore_ascii_case(value))
    {
        Ok(fallback)
    } else {
        parse_entry_rgb(entry)
    }
}

fn parse_optional_rgb(entry: &ThemeEntry) -> Result<Option<RgbColorDto>, String> {
    if entry.value.eq_ignore_ascii_case("none") {
        Ok(None)
    } else {
        parse_entry_rgb(entry).map(Some)
    }
}

fn parse_rgb(value: &str) -> Option<RgbColorDto> {
    let value = trim_quotes(value).trim();
    let hex = value.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let mut digits = hex.chars();
            let red = hex_digit(digits.next()?)?;
            let green = hex_digit(digits.next()?)?;
            let blue = hex_digit(digits.next()?)?;
            Some(RgbColorDto::new(red * 17, green * 17, blue * 17))
        }
        6 => Some(RgbColorDto::new(
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        _ => None,
    }
}

fn hex_digit(value: char) -> Option<u8> {
    value.to_digit(16).map(|value| value as u8)
}

fn trim_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn parses_kitty_palette_and_host_colors() {
        let entries = parse_kitty_theme(
            r#"
            # comment
            foreground #cdd6f4
            background #1e1e2e
            cursor #f5e0dc
            cursor_text_color background
            selection_foreground none
            selection_background #585b70
            color0 #45475a
            color15 #a6adc8
            active_tab_background #89b4fa
            "#,
            "test-theme.conf",
        );
        let config = ColorsConfig {
            theme: None,
            overrides: entries
                .into_iter()
                .map(|entry| (entry.key, entry.value))
                .collect::<BTreeMap<_, _>>(),
            resolved: TerminalColorTheme::default(),
        };

        let theme = resolve_color_theme(&config, Path::new("/tmp")).unwrap();
        assert_eq!(theme.foreground, RgbColorDto::new(205, 214, 244));
        assert_eq!(theme.background, RgbColorDto::new(30, 30, 46));
        assert_eq!(theme.cursor, RgbColorDto::new(245, 224, 220));
        assert_eq!(theme.cursor_text, theme.background);
        assert_eq!(theme.selection_foreground, None);
        assert_eq!(
            theme.selection_background,
            Some(RgbColorDto::new(88, 91, 112))
        );
        assert_eq!(theme.palette[0], RgbColorDto::new(69, 71, 90));
        assert_eq!(theme.palette[15], RgbColorDto::new(166, 173, 200));
        assert_eq!(theme.active_tab_background, RgbColorDto::new(137, 180, 250));
    }

    #[test]
    fn main_config_rejects_unknown_keys_and_accepts_short_hex() {
        let config = ColorsConfig {
            theme: None,
            overrides: BTreeMap::from([("foreground".to_string(), "#abc".to_string())]),
            resolved: TerminalColorTheme::default(),
        };
        assert_eq!(
            resolve_color_theme(&config, Path::new("/tmp"))
                .unwrap()
                .foreground,
            RgbColorDto::new(170, 187, 204)
        );

        let config = ColorsConfig {
            theme: None,
            overrides: BTreeMap::from([("unknown".to_string(), "#fff".to_string())]),
            resolved: TerminalColorTheme::default(),
        };
        assert!(
            resolve_color_theme(&config, Path::new("/tmp"))
                .unwrap_err()
                .contains("unsupported Kitty color key")
        );
    }
}
