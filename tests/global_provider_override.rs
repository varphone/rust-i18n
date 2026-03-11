use std::borrow::Cow;

struct AlphaBackend;
struct BetaBackend;

impl rust_i18n::Backend for AlphaBackend {
    fn available_locales(&self) -> Vec<Cow<'_, str>> {
        vec![Cow::Borrowed("alpha")]
    }

    fn translate(&self, locale: &str, key: &str) -> Option<Cow<'_, str>> {
        match (locale, key) {
            ("alpha", "hello") => Some(Cow::Borrowed("alpha.hello")),
            _ => None,
        }
    }
}

impl rust_i18n::Backend for BetaBackend {
    fn available_locales(&self) -> Vec<Cow<'_, str>> {
        vec![Cow::Borrowed("beta")]
    }

    fn translate(&self, locale: &str, key: &str) -> Option<Cow<'_, str>> {
        match (locale, key) {
            ("beta", "hello") => Some(Cow::Borrowed("beta.hello")),
            _ => None,
        }
    }
}

mod alpha_provider {
    rust_i18n::i18n!(backend = super::AlphaBackend);
}

mod beta_provider {
    rust_i18n::i18n!(backend = super::BetaBackend);
}

#[test]
fn test_set_global_provider_override() {
    let result = rust_i18n::set_global_provider("missing::provider");
    assert!(matches!(
        result,
        Err(rust_i18n::SetGlobalProviderError::UnknownProvider(_))
    ));

    let providers = rust_i18n::available_global_providers();
    let alpha = providers
        .iter()
        .copied()
        .find(|provider| provider.ends_with("alpha_provider"))
        .unwrap();
    let beta = providers
        .iter()
        .copied()
        .find(|provider| provider.ends_with("beta_provider"))
        .unwrap();

    assert_ne!(alpha, beta);
    assert_eq!(rust_i18n::global_provider(), None);

    rust_i18n::set_global_provider(beta).unwrap();
    assert_eq!(rust_i18n::available_locales!(), vec![Cow::Borrowed("beta")]);
    assert_eq!(rust_i18n::global_provider(), Some(beta));
    assert_eq!(rust_i18n::t!("hello", locale = "beta"), "beta.hello");
    assert_eq!(
        rust_i18n::set_global_provider(alpha),
        Err(rust_i18n::SetGlobalProviderError::AlreadyInitialized(beta))
    );
}
