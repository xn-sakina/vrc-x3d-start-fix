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
        Some(Language::ZhCn) => "zh-CN",
        Some(Language::En) => "en",
        None => normalize_locale(sys_locale::get_locale().as_deref()),
    };
    rust_i18n::set_locale(locale);
    locale
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
}
