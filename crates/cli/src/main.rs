use anyhow::Error;
use clap::{Args, Parser, Subcommand};
use indexmap::IndexMap;
use normpath::PathExt;
use rust_i18n_extract::extractor::Message;
use rust_i18n_extract::{extractor, generator, iter};
use rust_i18n_support::{I18nConfig, MinifyKey};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::{collections::HashMap, path::Path};

#[derive(Parser)]
#[command(name = "cargo")]
#[command(bin_name = "cargo")]
enum CargoCli {
    I18n(I18nArgs),
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
    /// The subcommand to run.
    #[command(subcommand)]
    cmd: Option<Commands>,

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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum MissedBehavior {
    #[default]
    Default,
    Empty,
}

impl FromStr for MissedBehavior {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(MissedBehavior::Default),
            "empty" => Ok(MissedBehavior::Empty),
            _ => Err("invalid missed behavior".to_string()),
        }
    }
}

#[derive(Debug, Args)]
struct I18nExportArgs {
    /// Specifies locales for the exported file. If not specified, all locales are
    /// included. Prefixes can be used:
    /// - `!` to exclude locales.
    /// - `+` to add extra locales.
    /// - no prefix to explicitly include locales, this priority is higher than `-`.
    ///
    /// For example, `-l en,+es` includes English and Spanish, excluding others.
    /// Even if Spanish is unavailable, it will be added to the exported file.
    /// Alternatively, `-l +es,!fr` includes all locales but French and adds Spanish.
    ///
    /// Each locale argument can be a comma-separated list, e.g. `-l en,+es,!fr`.
    #[arg(short = 'l', long, num_args(1..), value_delimiter=',', verbatim_doc_comment)]
    locales: Vec<String>,
    /// How to handle missing translations in the exported file.
    /// - `default`: Use the default value from the source file.
    /// - `empty`: Export an empty string for missing translations.
    #[arg(short = 'm', long, default_value = "default", verbatim_doc_comment)]
    missed: MissedBehavior,
    /// Specifies the output file for the exported i18n data.
    #[arg(short, long, default_value = "exported.csv")]
    output: String,
    /// Directory to look for `Cargo.toml` that includes `package.metadata.i18n`.
    #[arg(default_value = ".", last = true)]
    manifest_dir: Option<String>,
}

#[derive(Debug, Args)]
struct I18nSortArgs {
    /// Modify the loaded i18n file in-place, instead of creating a new one.
    #[arg(short, long, default_value_t = false)]
    inplace: bool,
    /// Reverse the sort order. Default is ascending.
    #[arg(short, long, default_value_t = false)]
    reverse: bool,
    /// Directory to look for `Cargo.toml` that includes `package.metadata.i18n`.
    #[arg(default_value = ".", last = true, verbatim_doc_comment)]
    manifest_dir: Option<String>,
}

/// The subcommands for the `cargo i18n` command.
#[derive(Subcommand)]
enum Commands {
    /// Export all translations to a single file
    ///
    /// The export format automatically detected from the output file extension.
    /// Supported formats are JSON, YAML, TOML, and CSV.
    ///
    /// The CSV format will have the following structure:
    /// ```csv
    /// key, en, es, fr
    /// "hello", "Hello", "Hola", "Bonjour"
    /// "world", "World", "Mundo", "Monde"
    /// ```
    #[clap(verbatim_doc_comment)]
    Export(I18nExportArgs),
    /// Sort i18n file by key and locale
    ///
    /// This command scans all i18n files in the locales directory, sorts them by
    /// key and locale, then writes the sorted content to a new file or overwrites
    /// the existing file if the `--inplace` flag is specified.
    #[clap(verbatim_doc_comment)]
    Sort(I18nSortArgs),
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

fn filter_locales(available_locales: &mut HashSet<String>, locales: &[String]) {
    let (explicit_locales, modifiers): (Vec<_>, Vec<_>) = locales
        .iter()
        .partition(|s| !(s.starts_with('+') || s.starts_with('!')));

    if !explicit_locales.is_empty() {
        available_locales.retain(|s| explicit_locales.contains(&s));
    }

    for locale in modifiers {
        let (prefix, locale) = locale.split_at(1);
        match prefix {
            "!" => {
                available_locales.remove(locale);
            }
            "+" => {
                available_locales.insert(locale.to_string());
            }
            _ => {}
        }
    }
}

fn i18n_export(args: I18nExportArgs) -> Result<(), Error> {
    let root = args.manifest_dir.unwrap_or(".".to_string());
    let config = I18nConfig::load(Path::new(&root))?;
    let load_path = find_load_path(&root, &config)?;
    let load_path_str = load_path.to_string_lossy();

    println!(r#"rust-i18n: loading locales from "{}" ..."#, load_path_str);

    let tmp_trs = rust_i18n_support::load_locales(&load_path_str, |_| false);
    for (locale, trs) in tmp_trs.iter() {
        println!(
            "rust-i18n: loaded {} translations for {}",
            trs.len(),
            locale
        );
    }

    let mut available_locales: HashSet<String> = config
        .available_locales
        .iter()
        .chain(tmp_trs.keys())
        .cloned()
        .collect();
    filter_locales(&mut available_locales, &args.locales);
    let mut sorted_locales: Vec<String> = available_locales.into_iter().collect();
    sorted_locales.sort();

    println!("rust-i18n: exporting locales: {:?}", sorted_locales);

    let keys: HashSet<_> = tmp_trs.iter().flat_map(|(_, map)| map.keys()).collect();
    let mut sorted_keys: Vec<&String> = keys.into_iter().collect();
    sorted_keys.sort();

    let mut new_trs: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
    for key in sorted_keys {
        let mut obj: IndexMap<String, String> = IndexMap::new();
        for locale in sorted_locales.iter() {
            let msg = tmp_trs.get(locale).and_then(|m| m.get(key));
            let msg = match (msg, args.missed) {
                (Some(msg), _) => msg.clone(),
                (None, MissedBehavior::Default) => tmp_trs
                    .get(&config.default_locale)
                    .and_then(|m| m.get(key))
                    .unwrap_or(&"".to_string())
                    .clone(),
                (None, MissedBehavior::Empty) => "".to_string(),
            };
            obj.insert(locale.clone(), msg);
        }
        new_trs.insert(key.clone(), obj);
    }

    let new_path = Path::new(&args.output);
    let ext = new_path
        .extension()
        .ok_or(anyhow::anyhow!("unexpected file format"))?
        .to_string_lossy();

    let text = convert_text(&new_trs, &ext)?;
    write_file(new_path, text)
        .map_err(|err| anyhow::anyhow!(r#"export to "{}" failed: {}"#, new_path.display(), err))?;

    println!(r#"rust-i18n: exported to "{}""#, new_path.display());

    Ok(())
}

fn convert_csv_text(trs: &IndexMap<String, IndexMap<String, String>>) -> Result<String, Error> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    let mut header = vec!["key".to_string()];
    if let Some(map) = trs.values().next() {
        header.extend(map.keys().cloned());
    }
    wtr.write_record(&header)?;
    for (key, val) in trs {
        let mut row = vec![key.clone()];
        for (_, text) in val {
            row.push(text.clone());
        }
        wtr.write_record(&row)?;
    }
    let text = String::from_utf8(wtr.into_inner()?)?;
    Ok(text)
}

fn convert_text(
    trs: &IndexMap<String, IndexMap<String, String>>,
    format: &str,
) -> Result<String, Error> {
    if format == "csv" {
        return convert_csv_text(trs);
    }

    let mut value = serde_json::Value::Object(serde_json::Map::new());
    value["_version"] = serde_json::Value::Number(serde_json::Number::from(2));

    for (key, val) in trs {
        let mut obj = serde_json::Value::Object(serde_json::Map::new());
        for (locale, text) in val {
            obj[locale] = serde_json::Value::String(text.clone());
        }
        value[key] = obj;
    }

    match format {
        "json" => Ok(serde_json::to_string_pretty(&value)?),
        "yaml" | "yml" => {
            let text = serde_yaml::to_string(&value)?;
            // Remove leading `---`
            Ok(text.trim_start_matches("---").trim_start().to_string())
        }
        "toml" => Ok(toml::to_string_pretty(&value)?),
        _ => Err(anyhow::anyhow!("unexpected file format: {}", format)),
    }
}

fn find_load_path(root: &str, config: &I18nConfig) -> Result<PathBuf, Error> {
    let load_path = Path::new(&config.load_path);
    let load_path = if load_path.is_absolute() {
        load_path.to_path_buf()
    } else {
        Path::new(&root).join(&config.load_path)
    };

    if load_path.exists() {
        let path = load_path.normalize()?;
        Ok(path.into_path_buf())
    } else {
        Err(anyhow::anyhow!(
            "missing load path: {}",
            load_path.display()
        ))
    }
}

fn write_file(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> Result<(), Error> {
    let mut output = ::std::fs::File::create(path)?;
    output.write_all(data.as_ref())?;
    Ok(())
}

fn i18n_sort(args: I18nSortArgs) -> Result<(), Error> {
    let root = args
        .manifest_dir
        .ok_or(anyhow::anyhow!("missing manifest directory"))?;
    let config = I18nConfig::load(Path::new(&root))?;
    let locales_path = find_load_path(&root, &config)?;
    let path_pattern = format!("{}/**/*.{{yml,yaml,json,toml}}", locales_path.display());

    for entry in globwalk::glob(path_pattern)? {
        let entry = entry.unwrap().into_path();
        if !args.inplace && entry.display().to_string().contains("-sorted") {
            continue;
        }

        println!(r#"rust-i18n: loading "{}" ..."#, entry.display());

        let tmp_trs = rust_i18n_support::load_locale(&entry);
        let available_locales: HashSet<_> = config
            .available_locales
            .iter()
            .chain(tmp_trs.keys())
            .collect();
        let mut sorted_locales: Vec<&String> = available_locales.into_iter().collect();
        sorted_locales.sort();

        let keys: HashSet<_> = tmp_trs.iter().flat_map(|(_, map)| map.keys()).collect();
        let mut sorted_keys: Vec<&String> = keys.into_iter().collect();
        sorted_keys.sort();

        if args.reverse {
            sorted_locales.reverse();
            sorted_keys.reverse();
        }

        let mut new_trs: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
        for key in sorted_keys {
            let mut obj: IndexMap<String, String> = IndexMap::new();
            for &locale in sorted_locales.iter() {
                if let Some(msg) = tmp_trs.get(locale).and_then(|m| m.get(key)) {
                    obj.insert(locale.clone(), msg.clone());
                }
            }
            new_trs.insert(key.clone(), obj);
        }

        let ext = entry.extension().unwrap().to_string_lossy();
        let new_path = if args.inplace {
            entry.to_string_lossy().to_string()
        } else {
            let mut new_path = entry.clone();
            new_path.set_file_name(format!(
                "{}-sorted.{}",
                entry.file_stem().unwrap().to_string_lossy(),
                ext
            ));
            new_path.to_string_lossy().to_string()
        };
        let text = convert_text(&new_trs, &ext)?;
        write_file(&new_path, &text)
            .map_err(|err| anyhow::anyhow!(r#"sort to "{}" failed: {}"#, &new_path, err))?;
        println!(r#"rust-i18n: sorted to "{}""#, &new_path);
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    let result = match CargoCli::parse() {
        CargoCli::I18n(args) => match args.cmd {
            Some(cmd) => match cmd {
                Commands::Export(args) => i18n_export(args),
                Commands::Sort(args) => i18n_sort(args),
            },
            None => i18n(args),
        },
    };

    if let Err(err) = result {
        eprintln!("rust-i18n: {}", err);
        std::process::exit(1);
    }

    Ok(())
}
