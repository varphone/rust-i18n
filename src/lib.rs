#![doc = include_str!("../README.md")]

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;
use std::sync::{LazyLock, OnceLock};

#[doc(hidden)]
pub use inventory;
#[cfg(feature = "log-miss-tr")]
#[doc(hidden)]
pub use log;
#[doc(hidden)]
pub use rust_i18n_macro::{_minify_key, _tr, i18n};
pub use rust_i18n_support::{
    try_load_locales, AtomicStr, Backend, BackendExt, CowStr, MinifyKey, SimpleBackend,
    StaticBackend, DEFAULT_MINIFY_KEY, DEFAULT_MINIFY_KEY_LEN, DEFAULT_MINIFY_KEY_PREFIX,
    DEFAULT_MINIFY_KEY_THRESH,
};

static CURRENT_LOCALE: LazyLock<AtomicStr> = LazyLock::new(|| AtomicStr::from("en"));
static GLOBAL_I18N_PROVIDER_OVERRIDE: OnceLock<&'static str> = OnceLock::new();
static GLOBAL_I18N_RUNTIME: OnceLock<Option<GlobalI18nRuntime>> = OnceLock::new();

#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct GlobalI18nOptions {
    pub fallback: Option<&'static [&'static str]>,
    pub minify_key: bool,
    pub minify_key_len: usize,
    pub minify_key_prefix: &'static str,
    pub minify_key_thresh: usize,
}

impl Default for GlobalI18nOptions {
    fn default() -> Self {
        Self {
            fallback: None,
            minify_key: DEFAULT_MINIFY_KEY,
            minify_key_len: DEFAULT_MINIFY_KEY_LEN,
            minify_key_prefix: DEFAULT_MINIFY_KEY_PREFIX,
            minify_key_thresh: DEFAULT_MINIFY_KEY_THRESH,
        }
    }
}

#[doc(hidden)]
pub struct GlobalI18nRegistration {
    pub backend: fn() -> &'static dyn Backend,
    pub options: GlobalI18nOptions,
    pub module_path: &'static str,
    pub is_primary_package: bool,
}

#[doc(hidden)]
inventory::collect!(GlobalI18nRegistration);

struct GlobalI18nRuntime {
    backend: &'static dyn Backend,
    options: GlobalI18nOptions,
    module_path: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetGlobalProviderError {
    UnknownProvider(String),
    AlreadyInitialized(&'static str),
    AlreadyOverridden(&'static str),
}

impl fmt::Display for SetGlobalProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProvider(provider) => {
                write!(f, "unknown global i18n provider: {provider}")
            }
            Self::AlreadyInitialized(provider) => {
                write!(f, "global i18n provider is already initialized: {provider}")
            }
            Self::AlreadyOverridden(provider) => {
                write!(
                    f,
                    "global i18n provider override is already set to: {provider}"
                )
            }
        }
    }
}

impl std::error::Error for SetGlobalProviderError {}

fn global_i18n_registrations() -> Vec<&'static GlobalI18nRegistration> {
    inventory::iter::<GlobalI18nRegistration>
        .into_iter()
        .collect::<Vec<_>>()
}

fn is_crate_root_module(module_path: &str) -> bool {
    !module_path.contains("::")
}

fn registration_priority(registration: &GlobalI18nRegistration) -> (u8, usize) {
    let rank = match (
        registration.is_primary_package,
        is_crate_root_module(registration.module_path),
    ) {
        (true, true) => 3,
        (false, true) => 2,
        (true, false) => 1,
        (false, false) => 0,
    };

    (rank, usize::MAX - registration.module_path.len())
}

fn default_global_i18n_registration() -> Option<&'static GlobalI18nRegistration> {
    global_i18n_registrations()
        .into_iter()
        .max_by_key(|registration| registration_priority(registration))
}

fn global_i18n_registration_by_path(module_path: &str) -> Option<&'static GlobalI18nRegistration> {
    global_i18n_registrations()
        .into_iter()
        .find(|registration| registration.module_path == module_path)
}

fn selected_global_i18n_registration() -> Option<&'static GlobalI18nRegistration> {
    GLOBAL_I18N_PROVIDER_OVERRIDE
        .get()
        .and_then(|module_path| global_i18n_registration_by_path(module_path))
        .or_else(default_global_i18n_registration)
}

fn global_i18n_runtime() -> Option<&'static GlobalI18nRuntime> {
    GLOBAL_I18N_RUNTIME
        .get_or_init(|| {
            selected_global_i18n_registration().map(|registration| GlobalI18nRuntime {
                backend: (registration.backend)(),
                options: registration.options,
                module_path: registration.module_path,
            })
        })
        .as_ref()
}

/// Return all discovered global i18n providers.
pub fn available_global_providers() -> Vec<&'static str> {
    let mut providers = global_i18n_registrations()
        .into_iter()
        .map(|registration| registration.module_path)
        .collect::<Vec<_>>();
    providers.sort_unstable();
    providers.dedup();
    providers
}

/// Return the selected global i18n provider if one has been explicitly fixed.
pub fn global_provider() -> Option<&'static str> {
    GLOBAL_I18N_RUNTIME
        .get()
        .and_then(|runtime| runtime.as_ref())
        .map(|runtime| runtime.module_path)
        .or_else(|| GLOBAL_I18N_PROVIDER_OVERRIDE.get().copied())
}

/// Override the global i18n provider before the runtime is first used.
pub fn set_global_provider(module_path: &'static str) -> Result<(), SetGlobalProviderError> {
    if let Some(runtime) = GLOBAL_I18N_RUNTIME
        .get()
        .and_then(|runtime| runtime.as_ref())
    {
        return if runtime.module_path == module_path {
            Ok(())
        } else {
            Err(SetGlobalProviderError::AlreadyInitialized(
                runtime.module_path,
            ))
        };
    }

    if global_i18n_registration_by_path(module_path).is_none() {
        return Err(SetGlobalProviderError::UnknownProvider(
            module_path.to_string(),
        ));
    }

    if let Some(current) = GLOBAL_I18N_PROVIDER_OVERRIDE.get() {
        return if *current == module_path {
            Ok(())
        } else {
            Err(SetGlobalProviderError::AlreadyOverridden(current))
        };
    }

    GLOBAL_I18N_PROVIDER_OVERRIDE
        .set(module_path)
        .map_err(SetGlobalProviderError::AlreadyOverridden)
}

#[doc(hidden)]
pub fn _rust_i18n_lookup_fallback(locale: &str) -> Option<&str> {
    locale
        .rfind('-')
        .map(|n| locale[..n].trim_end_matches("-x"))
}

#[doc(hidden)]
pub fn _rust_i18n_global_options() -> GlobalI18nOptions {
    global_i18n_runtime()
        .map(|runtime| runtime.options)
        .unwrap_or_default()
}

#[doc(hidden)]
pub fn _rust_i18n_try_translate(locale: &str, key: impl AsRef<str>) -> Option<Cow<'static, str>> {
    let runtime = global_i18n_runtime()?;
    let key = key.as_ref();

    runtime.backend.translate(locale, key).or_else(|| {
        let mut current_locale = locale;
        while let Some(fallback_locale) = _rust_i18n_lookup_fallback(current_locale) {
            if let Some(value) = runtime.backend.translate(fallback_locale, key) {
                return Some(value);
            }
            current_locale = fallback_locale;
        }

        runtime.options.fallback.and_then(|fallback| {
            fallback
                .iter()
                .find_map(|locale| runtime.backend.translate(locale, key))
        })
    })
}

#[doc(hidden)]
pub fn _rust_i18n_translate<'r>(locale: &str, key: &'r str) -> Cow<'r, str> {
    _rust_i18n_try_translate(locale, key).unwrap_or_else(|| {
        if locale.is_empty() {
            key.into()
        } else {
            format!("{}.{}", locale, key).into()
        }
    })
}

#[doc(hidden)]
pub fn _rust_i18n_maybe_minify_key<'r>(value: &'r str) -> Cow<'r, str> {
    let options = _rust_i18n_global_options();
    if options.minify_key {
        MinifyKey::minify_key(
            value,
            options.minify_key_len,
            options.minify_key_prefix,
            options.minify_key_thresh,
        )
    } else {
        Cow::Borrowed(value)
    }
}

/// Get all available locales from the selected global i18n provider.
pub fn available_locales() -> Vec<Cow<'static, str>> {
    if let Some(runtime) = global_i18n_runtime() {
        let mut locales = runtime.backend.available_locales();
        locales.sort();
        locales
    } else {
        Vec::new()
    }
}

/// Set current locale
pub fn set_locale(locale: &str) {
    CURRENT_LOCALE.replace(locale);
}

/// Get current locale
pub fn locale() -> impl Deref<Target = str> {
    CURRENT_LOCALE.as_str()
}

/// Replace patterns and return a new string.
///
/// # Arguments
///
/// * `input` - The input string, containing patterns like `%{name}`.
/// * `patterns` - The patterns to replace.
/// * `values` - The values to replace.
///
/// # Example
///
/// ```
/// # use rust_i18n::replace_patterns;
/// let input = "Hello, %{name}!";
/// let patterns = &["name"];
/// let values = &["world".to_string()];
/// let output = replace_patterns(input, patterns, values);
/// assert_eq!(output, "Hello, world!");
/// ```
pub fn replace_patterns(input: &str, patterns: &[&str], values: &[String]) -> String {
    let input_bytes = input.as_bytes();
    let mut pattern_pos = smallvec::SmallVec::<[usize; 64]>::new();
    let mut stage = 0;
    for (i, &b) in input_bytes.iter().enumerate() {
        match (stage, b) {
            (1, b'{') => {
                stage = 2;
                pattern_pos.push(i);
            }
            (2, b'}') => {
                stage = 0;
                pattern_pos.push(i);
            }
            (_, b'%') => {
                stage = 1;
            }
            _ => {}
        }
    }
    let mut output: Vec<u8> = Vec::with_capacity(input_bytes.len() + 128);
    let mut prev_end = 0;
    let pattern_values = patterns.iter().zip(values.iter());
    for pos in pattern_pos.chunks_exact(2) {
        let start = pos[0];
        let end = pos[1];
        let key = &input_bytes[start + 1..end];
        if prev_end < start {
            let prev_chunk = &input_bytes[prev_end..start - 1];
            output.extend_from_slice(prev_chunk);
        }
        if let Some((_, v)) = pattern_values
            .clone()
            .find(|(&pattern, _)| pattern.as_bytes() == key)
        {
            output.extend_from_slice(v.as_bytes());
        } else {
            output.extend_from_slice(&input_bytes[start - 1..end + 1]);
        }
        prev_end = end + 1;
    }
    if prev_end < input_bytes.len() {
        let remaining = &input_bytes[prev_end..];
        output.extend_from_slice(remaining);
    }
    unsafe { String::from_utf8_unchecked(output) }
}

/// Get I18n text
///
/// This macro forwards to the global runtime, which is discovered from crates that expanded [`i18n!`].
///
/// # Arguments
///
/// * `expr` - The key or message for translation.
///   - A key usually looks like `"foo.bar.baz"`.
///   - A literal message usually looks like `"Hello, world!"`.
///   - The variable names in the message should be wrapped in `%{}`, like `"Hello, %{name}!"`.
///   - Dynamic messages are also supported, such as `t!(format!("Hello, {}!", name))`.
///     However, if `minify_key` is enabled, the entire message will be hashed and used as a key for every lookup, which may consume more CPU cycles.
/// * `locale` - The locale to use. If not specified, the current locale will be used.
/// * `args` - The arguments to be replaced in the translated text.
///    - These should be passed in the format `key = value` or `key => value`.
///    - Alternatively, you can specify the value format using the `key = value : {:format_specifier}` syntax.
///      For example, `key = value : {:08}` will format the value as a zero-padded string with a length of 8.
///
/// # Example
///
/// ```no_run
/// #[macro_use] extern crate rust_i18n;
///
/// # macro_rules! t { ($($all:tt)*) => {} }
/// # fn main() {
/// // Simple get text with current locale
/// t!("greeting");
/// // greeting: "Hello world" => "Hello world"
///
/// // Get a special locale's text
/// t!("greeting", locale = "de");
/// // greeting: "Hallo Welt!" => "Hallo Welt!"
///
/// // With variables
/// t!("messages.hello", name = "world");
/// // messages.hello: "Hello, %{name}" => "Hello, world"
/// t!("messages.foo", name = "Foo", other ="Bar");
/// // messages.foo: "Hello, %{name} and %{other}" => "Hello, Foo and Bar"
///
/// // With variables and format specifiers
/// t!("Hello, %{name}, you serial number is: %{sn}", name = "Jason", sn = 123 : {:08});
/// // => "Hello, Jason, you serial number is: 000000123"
///
/// // With locale and variables
/// t!("messages.hello", locale = "de", name = "Jason");
/// // messages.hello: "Hallo, %{name}" => "Hallo, Jason"
/// # }
/// ```
#[macro_export]
#[allow(clippy::crate_in_macro_def)]
macro_rules! t {
    ($($all:tt)*) => {
        rust_i18n::_tr!($($all)*)
    }
}

/// A macro that generates a translation key and corresponding value pair from a given input value.
///
/// It's useful when you want to use a long string as a key, but you don't want to type it twice.
///
/// # Arguments
///
/// * `msg` - The input value.
///
/// # Returns
///
/// A tuple of `(key, msg)`.
///
/// # Example
///
/// ```no_run
/// use rust_i18n::{t, tkv};
///
/// # macro_rules! t { ($($all:tt)*) => { } }
/// # macro_rules! tkv { ($($all:tt)*) => { (1,2) } }
///
/// let (key, msg) = tkv!("Hello world");
/// // => key is `"Hello world"` and msg is the translated message.
/// // => If there is hints the minify_key logic, the key will returns a minify key.
/// ```
#[macro_export]
#[allow(clippy::crate_in_macro_def)]
macro_rules! tkv {
    ($msg:literal) => {{
        let val = $msg;
        let key = rust_i18n::_rust_i18n_maybe_minify_key(val);
        (key, val)
    }};
}

/// Get available locales
///
/// ```no_run
/// #[macro_use] extern crate rust_i18n;
/// # pub fn _rust_i18n_available_locales() -> Vec<&'static str> { todo!() }
/// # fn main() {
/// rust_i18n::available_locales!();
/// # }
/// // => ["en", "zh-CN"]
/// ```
#[macro_export(local_inner_macros)]
#[allow(clippy::crate_in_macro_def)]
macro_rules! available_locales {
    () => {
        rust_i18n::available_locales()
    };
}

#[cfg(test)]
mod tests {
    use crate::{locale, CURRENT_LOCALE};

    fn assert_locale_type(s: &str, val: &str) {
        assert_eq!(s, val);
    }

    #[test]
    fn test_locale() {
        assert_locale_type(&locale(), &CURRENT_LOCALE.as_str());
        assert_eq!(&*locale(), "en");
    }
}
