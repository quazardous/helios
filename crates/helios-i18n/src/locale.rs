//! Which locale to use, and the chain a lookup falls back along.

use unic_langid::LanguageIdentifier;

/// The environment variable that overrides every other choice.
pub const ENV: &str = "HELIOS_LANG";

/// The source language: the text written in the code is English.
pub const ENGLISH: &str = "en";

/// The pseudo-locale: English, accented and lengthened, to see untranslated
/// text and layouts that do not absorb longer strings.
pub const PSEUDO: &str = "x-pseudo";

/// A locale tag in its canonical form (`fr-CA`, `x-pseudo`), from a tag or
/// a POSIX locale name (`fr_CA.UTF-8@euro`); `None` when it names no
/// language. `C` and `POSIX` are English.
pub fn normalize(tag: &str) -> Option<String> {
    let tag = tag.trim();
    let tag = tag.split(['.', '@']).next().unwrap_or_default();
    if tag.is_empty() {
        return None;
    }
    if tag == "C" || tag == "POSIX" {
        return Some(ENGLISH.into());
    }
    let tag = tag.replace('_', "-");
    if tag.eq_ignore_ascii_case(PSEUDO) {
        return Some(PSEUDO.into());
    }
    tag.parse::<LanguageIdentifier>()
        .ok()
        .filter(|l| l.language.as_str() != "und")
        .map(|l| l.to_string())
}

/// The locale to use: `env` (the value of `HELIOS_LANG`), then `setting`
/// (the user's choice), then `system`, then English. A value that names no
/// language is skipped.
pub fn choose(env: Option<&str>, setting: Option<&str>, system: Option<&str>) -> String {
    [env, setting, system]
        .into_iter()
        .flatten()
        .find_map(normalize)
        .unwrap_or_else(|| ENGLISH.into())
}

/// [`choose`] with `HELIOS_LANG` and the system's language. Tests do not
/// call it: they pin English (`I18n::english`).
pub fn detect(setting: Option<&str>) -> String {
    let env = std::env::var(ENV).ok();
    let system = sys_locale::get_locale();
    choose(env.as_deref(), setting, system.as_deref())
}

/// The locales a lookup tries, most specific first, ending with English:
/// `zh-Hant-TW` gives `zh-Hant-TW`, `zh-Hant`, `zh`, `en`. The
/// pseudo-locale reads English.
pub fn chain(locale: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if let Some(mut l) = (locale != PSEUDO)
        .then(|| locale.parse::<LanguageIdentifier>().ok())
        .flatten()
    {
        push(l.to_string());
        l.clear_variants();
        push(l.to_string());
        l.region = None;
        push(l.to_string());
        l.script = None;
        push(l.to_string());
    }
    push(ENGLISH.into());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_wins_then_the_setting_then_the_system() {
        assert_eq!(choose(Some("fr_FR.UTF-8"), Some("de"), Some("it")), "fr-FR");
        assert_eq!(choose(None, Some("de"), Some("it-IT")), "de");
        assert_eq!(choose(Some(""), None, Some("it-IT")), "it-IT");
        assert_eq!(choose(Some("??"), Some(" "), None), "en");
        assert_eq!(choose(Some("C"), Some("de"), None), "en");
        assert_eq!(choose(Some("x-pseudo"), None, None), PSEUDO);
    }

    #[test]
    fn the_chain_drops_subtags_then_ends_in_english() {
        assert_eq!(chain("fr-CA"), ["fr-CA", "fr", "en"]);
        assert_eq!(chain("zh-Hant-TW"), ["zh-Hant-TW", "zh-Hant", "zh", "en"]);
        assert_eq!(chain("en-GB"), ["en-GB", "en"]);
        assert_eq!(chain(PSEUDO), ["en"]);
    }
}
