use rust_i18n::t;

pub fn hello() -> String {
    t!("hello").into_owned()
}

pub fn translated_literal(locale: &str) -> String {
    t!("Bar - Hello, World!", locale = locale).into_owned()
}

pub fn locales() -> Vec<String> {
    rust_i18n::available_locales!()
        .into_iter()
        .map(|locale| locale.into_owned())
        .collect()
}
