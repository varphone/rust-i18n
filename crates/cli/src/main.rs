use anyhow::Error;
use clap::{Args, Parser};
use rust_i18n_extract::extractor::Message;
use rust_i18n_extract::{extractor, generator, iter};
use rust_i18n_support::{I18nConfig, MinifyKey};
use std::collections::HashSet;
use std::{collections::HashMap, path::Path};

#[derive(Parser)]
#[command(name = "cargo")]
#[command(bin_name = "cargo")]
enum CargoCli {
    I18n(I18nArgs),
    I18nExport(I18nExportArgs),
}

#[derive(Args)]
#[command(author, version)]
// #[command(propagate_version = true)]
/// Rust I18n command to help you extract all untranslated texts from source code.
///
/// It will iterate all Rust files in the source directory and extract all untranslated texts
/// that used `t!` macro.
/// Then it will generate a YAML file and merge with the existing translations.
///
/// https://github.com/longbridgeapp/rust-i18n
struct I18nArgs {
    /// Manually add a translation to the localization file.
    ///
    /// This is useful for non-literal values in the `t!` macro.
    ///
    /// For example, if you have `t!(format!("Hello, {}!", "world"))` in your code,
    /// you can add a translation for it using `-t "Hello, world!"`,
    /// or provide a translated message using `-t "Hello, world! => Hola, world!"`.
    ///
    /// NOTE: The whitespace before and after the key and value will be trimmed.
    #[arg(short, long, default_value = None, name = "TEXT", num_args(1..), value_parser = translate_value_parser, verbatim_doc_comment)]
    translate: Option<Vec<(String, String)>>,
    /// Extract all untranslated I18n texts from source code
    #[arg(default_value = "./", last = true)]
    source: Option<String>,
}

/// Export I18n to a file with the given format.
#[derive(Args)]
#[command(author, version)]
struct I18nExportArgs {
    /// The extra locales to export the I18n to.
    #[arg(short = 'l', long, num_args(0..))]
    extra_locales: Vec<String>,
    /// The format to export the I18n to.
    #[arg(short, long, default_value = "csv")]
    format: Option<String>,
    /// The output file to export the I18n to.
    #[arg(short, long, default_value = "exported")]
    output: String,
}

/// Remove quotes from a string at the start and end.
fn remove_quotes(s: &str) -> &str {
    let mut start = 0;
    let mut end = s.len();
    if s.starts_with('"') {
        start += 1;
    }
    if s.ends_with('"') {
        end -= 1;
    }
    &s[start..end]
}

/// Parse a string of the form "key => value" into a tuple.
fn translate_value_parser(s: &str) -> Result<(String, String), std::io::Error> {
    if let Some((key, msg)) = s.split_once("=>") {
        let key = remove_quotes(key.trim());
        let msg = remove_quotes(msg.trim());
        Ok((key.to_owned(), msg.to_owned()))
    } else {
        Ok((s.to_owned(), s.to_owned()))
    }
}

/// Add translations to the localize file for t!
fn add_translations(
    list: &[(String, String)],
    results: &mut HashMap<String, Message>,
    cfg: &I18nConfig,
) {
    let I18nConfig {
        minify_key,
        minify_key_len,
        minify_key_prefix,
        minify_key_thresh,
        ..
    } = cfg;

    for item in list {
        let index = results.len();
        let key = if *minify_key {
            let hashed_key =
                item.0
                    .minify_key(*minify_key_len, minify_key_prefix, *minify_key_thresh);
            hashed_key.to_string()
        } else {
            item.0.clone()
        };
        results.entry(key).or_insert(Message {
            key: item.1.clone(),
            index,
            minify_key: *minify_key,
            locations: vec![],
        });
    }
}

fn i18n(args: I18nArgs) -> Result<(), Error> {
    let mut results = HashMap::new();

    let source_path = args.source.expect("Missing source path");

    let cfg = I18nConfig::load(std::path::Path::new(&source_path))?;

    iter::iter_crate(&source_path, |path, source| {
        extractor::extract(&mut results, path, source, cfg.clone())
    })?;

    if let Some(list) = args.translate {
        add_translations(&list, &mut results, &cfg);
    }

    let mut messages: Vec<_> = results.iter().collect();
    messages.sort_by_key(|(_k, m)| m.index);

    let output_path = Path::new(&source_path).join(&cfg.load_path);

    generator::generate(output_path, &cfg.available_locales, messages.clone())?;

    Ok(())
}

fn i18n_export(args: I18nExportArgs) -> Result<(), Error> {
    let format = args.format.expect("Missing format");
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap_or(".".to_string());
    if let Ok(config) = I18nConfig::load(Path::new(&root)) {
        let empty_str = String::new();
        let key = "key".to_string();

        // Load all translations
        let translations = rust_i18n_support::load_locales(&config.load_path, |_| false);

        // Get all available locales
        let available_locales: HashSet<_> = config
            .available_locales
            .iter()
            .chain(args.extra_locales.iter())
            .chain(translations.keys())
            .collect();
        let mut sorted_locales: Vec<&String> = available_locales.into_iter().collect();
        sorted_locales.sort();
        sorted_locales.insert(0, &key);

        // Get all keys
        let keys: HashSet<_> = translations
            .iter()
            .flat_map(|(_, map)| map.keys())
            .collect();
        let mut sorted_keys: Vec<&String> = keys.into_iter().collect();
        sorted_keys.sort();

        // Write to file
        let output_file = format!("{}.csv", args.output);
        let mut csv_writer = csv::WriterBuilder::new()
            .has_headers(true)
            .from_path(output_file)?;

        // Write header
        csv_writer.write_record(&sorted_locales)?;

        // Write rows
        for key in sorted_keys {
            let mut row = vec![key];
            for locale in sorted_locales.iter().skip(1) {
                let msg: Option<&String> = translations.get(*locale).and_then(|m| m.get(key));
                row.push(msg.unwrap_or(&empty_str));
            }
            csv_writer.write_record(&row)?;
        }
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    let result = match CargoCli::parse() {
        CargoCli::I18n(args) => i18n(args),
        CargoCli::I18nExport(args) => i18n_export(args),
    };

    if result.is_err() {
        std::process::exit(1);
    }

    Ok(())
}
