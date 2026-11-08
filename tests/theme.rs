use ratatui::style::{Color, Modifier};
use rdp_tui::config::theme::{ThemeColor, ThemeRole, parse_theme_document};
use rdp_tui::tui::theme::Theme;
use tempfile::TempDir;

fn theme(text: &str) -> Theme {
    Theme::from_document(&parse_theme_document(text).unwrap())
}

#[test]
fn partial_overrides_distinguish_inheritance_reset_and_empty_modifiers() {
    let resolved = theme(
        "version = 1\n[styles.text]\nfg = '#123456'\n[styles.selection]\nmodifiers = []\n[styles.error]\nfg = 'default'\n",
    );
    assert_eq!(resolved.text.fg, Some(Color::Rgb(0x12, 0x34, 0x56)));
    assert_eq!(resolved.title.fg, None);
    assert_eq!(resolved.error.fg, Some(Color::Reset));
    assert!(!resolved.selection.add_modifier.contains(Modifier::REVERSED));
    assert!(resolved.selection.sub_modifier.contains(Modifier::REVERSED));
}

#[test]
fn palette_aliases_and_new_rgb_themes_are_runtime_data() {
    let text = "version = 1\n[palette]\ncustom = '#abcdef'\n[styles.warning]\nfg = '$custom'\n";
    let document = parse_theme_document(text).unwrap();
    assert_eq!(
        document.styles[&ThemeRole::Warning].fg,
        Some(ThemeColor::Rgb(0xab, 0xcd, 0xef))
    );
    let resolved = Theme::from_document(&document);
    assert_eq!(resolved.warning.fg, Some(Color::Rgb(0xab, 0xcd, 0xef)));
    assert!(resolved.warning.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn explicit_selection_colors_remove_inherited_reverse_and_dim() {
    let resolved = theme(
        "version = 1\n[styles.text]\nfg = '#eeeeee'\nbg = '#111111'\n[styles.selection]\nbg = '#ffffff'\n",
    );
    assert_eq!(resolved.selection.fg, Some(Color::Rgb(0xee, 0xee, 0xee)));
    assert_eq!(resolved.selection.bg, Some(Color::Rgb(255, 255, 255)));
    assert!(!resolved.selection.add_modifier.contains(Modifier::REVERSED));
    assert!(
        resolved
            .selection
            .sub_modifier
            .contains(Modifier::REVERSED | Modifier::DIM)
    );
}

#[test]
fn rejects_invalid_configuration_without_dumping_document_contents() {
    let invalid = [
        "version = 2",
        "version = 1\npreset = 'brogrammer'",
        "version = 1\n[styles.typo]\nfg = 'red'",
        "version = 1\n[styles.error]\nforeground = 'red'",
        "version = 1\n[styles.error]\nfg = '#12345'",
        "version = 1\n[styles.error]\nfg = '#ééé'",
        "version = 1\n[styles.error]\nfg = '$missing'",
        "version = 1\n[palette]\nred = '$other'",
        "version = 1\n[styles.error]\nmodifiers = ['hidden']",
        "version = 1\n[styles.error]\nmodifiers = ['blink']",
        "version = 1\n[styles.error]\nfg = 42",
        "version = 1\n# sensitive-marker\n[styles.error\n",
    ];
    for text in invalid {
        let error = parse_theme_document(text).unwrap_err().to_string();
        assert!(!error.contains("sensitive-marker"));
        assert!(!error.is_empty());
    }
}

#[test]
fn accepts_all_documented_ansi_colors_and_modifiers() {
    for name in [
        "default",
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "gray",
        "dark_gray",
        "light_red",
        "light_green",
        "light_yellow",
        "light_blue",
        "light_magenta",
        "light_cyan",
        "white",
    ] {
        let text = format!(
            "version = 1\n[styles.text]\nfg = '{name}'\nmodifiers = ['bold', 'dim', 'italic', 'underline', 'reversed']\n"
        );
        let resolved = theme(&text);
        assert!(resolved.text.fg.is_some());
        assert!(resolved.text.add_modifier.contains(
            Modifier::BOLD
                | Modifier::DIM
                | Modifier::ITALIC
                | Modifier::UNDERLINED
                | Modifier::REVERSED
        ));
    }
}

#[test]
fn loading_is_read_only_and_warns_instead_of_preventing_startup() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("theme.toml");
    let (missing, warning) = Theme::load(root.path());
    assert!(warning.is_none());
    assert!(missing.selection.add_modifier.contains(Modifier::REVERSED));
    assert!(!path.exists());
    std::fs::write(&path, "version = 999\n").unwrap();
    let (_, warning) = Theme::load(root.path());
    let warning = warning.unwrap();
    assert!(warning.contains(path.to_str().unwrap()));
    assert!(warning.contains("version"));
    assert!(warning.contains("Using monochrome"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = 999\n");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(Theme::load(root.path()).1.is_some());
}

fn luminance(color: Color) -> f64 {
    let Color::Rgb(red, green, blue) = color else {
        panic!("RGB expected")
    };
    let linear = |value: u8| {
        let channel = f64::from(value) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn contrast(fg: Color, bg: Color) -> f64 {
    let (a, b) = (luminance(fg), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn shipped_themes_parse_and_brogrammer_text_pairs_have_readable_contrast() {
    for file in ["brogrammer", "terminal", "monochrome"] {
        let text =
            std::fs::read_to_string(format!("{}/themes/{file}.toml", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        assert!(parse_theme_document(&text).is_ok());
    }
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/themes/brogrammer.toml"
    ))
    .unwrap();
    let resolved = theme(&text);
    let background = resolved.text.bg.unwrap();
    for style in [
        resolved.text,
        resolved.text_muted,
        resolved.info,
        resolved.success,
        resolved.warning,
        resolved.error,
    ] {
        let ratio = contrast(style.fg.unwrap(), style.bg.unwrap_or(background));
        assert!(ratio >= 4.5, "contrast {ratio} is below 4.5:1");
    }
    for style in [resolved.selection, resolved.field_focused] {
        assert!(contrast(style.fg.unwrap(), style.bg.unwrap()) >= 4.5);
        assert!(!style.add_modifier.contains(Modifier::REVERSED));
    }
}

#[test]
fn reversed_selection_uses_base_pair_and_does_not_inherit_dim() {
    let resolved =
        theme("version = 1\n[styles.text]\nfg = '#abcdef'\nbg = '#102030'\nmodifiers = ['dim']\n");
    for style in [resolved.selection, resolved.field_focused] {
        assert_eq!(style.fg, resolved.text.fg);
        assert_eq!(style.bg, resolved.text.bg);
        assert!(style.add_modifier.contains(Modifier::REVERSED));
        assert!(style.sub_modifier.contains(Modifier::DIM));
    }
}

#[test]
fn relative_reverse_selection_inverts_an_already_reversed_base() {
    let resolved = theme("version = 1\n[styles.text]\nmodifiers = ['reversed']\n");
    for style in [resolved.selection, resolved.field_focused] {
        assert!(!style.add_modifier.contains(Modifier::REVERSED));
        assert!(style.sub_modifier.contains(Modifier::REVERSED));
    }
}
