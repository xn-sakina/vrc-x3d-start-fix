use crate::config::Language;

pub fn normalize_locale(locale: Option<&str>) -> &'static str {
    match locale.map(|value| value.to_ascii_lowercase()) {
        Some(value) if value == "zh" || value.starts_with("zh-") || value.starts_with("zh_") => {
            "zh-CN"
        }
        _ => "en",
    }
}

pub fn initialize(override_language: Option<Language>) -> &'static str {
    let locale = match override_language {
        Some(language) => language.locale(),
        None => normalize_locale(sys_locale::get_locale().as_deref()),
    };
    rust_i18n::set_locale(locale);
    locale
}

pub fn language_for_locale(locale: &str) -> Language {
    match normalize_locale(Some(locale)) {
        "zh-CN" => Language::ZhCn,
        _ => Language::En,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_variants_select_simplified_chinese() {
        for value in ["zh", "zh-CN", "zh_SG", "zh-Hans", "ZH-tw"] {
            assert_eq!(normalize_locale(Some(value)), "zh-CN");
        }
    }

    #[test]
    fn everything_else_falls_back_to_english() {
        assert_eq!(normalize_locale(Some("en-US")), "en");
        assert_eq!(normalize_locale(Some("ja-JP")), "en");
        assert_eq!(normalize_locale(None), "en");
    }

    #[test]
    fn effective_language_matches_normalized_locale() {
        assert_eq!(language_for_locale("zh-Hans"), Language::ZhCn);
        assert_eq!(language_for_locale("en-US"), Language::En);
    }

    #[test]
    fn tray_tooltips_stay_short_and_single_line() {
        let status_keys = [
            "tooltip.waiting",
            "tooltip.detected",
            "tooltip.disturbing",
            "tooltip.success",
            "tooltip.failed",
            "tooltip.unknown",
            "tooltip.shutting_down",
        ];

        for locale in ["en", "zh-CN"] {
            for status_key in status_keys {
                let status = rust_i18n::t!(status_key, locale = locale);
                let tooltip = rust_i18n::t!("tooltip.format", locale = locale, status = status);
                assert!(!tooltip.contains(['\n', '\r']), "{locale}: {tooltip}");
                let app_name = rust_i18n::t!("app.name", locale = locale);
                assert!(
                    tooltip.starts_with(app_name.as_ref()),
                    "{locale}: {tooltip}"
                );
                assert!(tooltip.chars().count() <= 48, "{locale}: {tooltip}");
            }
        }
    }
}
