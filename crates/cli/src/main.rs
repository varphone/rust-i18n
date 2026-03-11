use anstyle::{AnsiColor, Color, Style};
use anyhow::Error;
use clap::{Args, Parser, Subcommand};
use indexmap::IndexMap;
use normpath::PathExt;
use rust_i18n_extract::extractor::Message;
use rust_i18n_extract::{extractor, generator, iter};
use rust_i18n_support::{I18nConfig, MinifyKey};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

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
/// https://github.com/longbridge/rust-i18n
struct I18nArgs {
    /// The subcommand to run.
    #[command(subcommand)]
    cmd: Option<Commands>,

    /// Recursively scan Cargo dependencies, including workspace members, local path
    /// dependencies and registry crates that depend on rust-i18n.
    #[arg(long, default_value_t = false, verbatim_doc_comment)]
    include_deps: bool,

    /// When used with --include-deps, skip registry dependencies and only scan
    /// workspace members plus local path dependencies.
    #[arg(
        long,
        default_value_t = false,
        requires = "include_deps",
        verbatim_doc_comment
    )]
    local_deps_only: bool,

    /// When used with --include-deps, skip dependency packages with the given
    /// package names. Repeat the flag or use a comma-separated list.
    #[arg(long, value_name = "NAME", requires = "include_deps", value_delimiter = ',', num_args(1..), verbatim_doc_comment)]
    exclude_package: Vec<String>,

    /// When used with --include-deps, only scan dependency packages with the
    /// given package names. Repeat the flag or use a comma-separated list.
    #[arg(long, value_name = "NAME", requires = "include_deps", value_delimiter = ',', num_args(1..), verbatim_doc_comment)]
    include_package: Vec<String>,

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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Lints {
    #[default]
    Unused,
}

impl FromStr for Lints {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unused" => Ok(Lints::Unused),
            _ => Err("invalid lint".to_string()),
        }
    }
}

#[derive(Debug, Args)]
struct I18nLintArgs {
    /// Specifies the lints to execute. Currently, only the `unused` lint is supported.
    #[arg(short = 'l', long, default_value = "unused", verbatim_doc_comment)]
    lints: Lints,
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
    #[clap(verbatim_doc_comment)]
    /// Run lints on the i18n files
    ///
    /// This command scans all i18n files in the locales directory and runs lints
    /// on them. Currently, the only lint available is `unused`, which checks for
    /// unused translations in the i18n files.
    Lint(I18nLintArgs),
    /// Sort i18n file by key and locale
    ///
    /// This command scans all i18n files in the locales directory, sorts them by
    /// key and locale, then writes the sorted content to a new file or overwrites
    /// the existing file if the `--inplace` flag is specified.
    #[clap(verbatim_doc_comment)]
    Sort(I18nSortArgs),
}

const ERROR_STYLE: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
const IDENT_STYLE: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
const KEY_STYLE: Style = Style::new().bold();
const PATH_STYLE: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const WARN_STYLE: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

macro_rules! msg {
    (ERROR, $msg:expr) => {
        eprintln!("{ERROR_STYLE}rust-i18n{ERROR_STYLE:#}: {msg}", msg = $msg);
    };
    (WARN, $msg:expr) => {
        eprintln!("{WARN_STYLE}rust-i18n{WARN_STYLE:#}: {msg}", msg = $msg);
    };
    (EXPORTED_TO, $path:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: exported to {PATH_STYLE}{}{PATH_STYLE:#}", $path);
    };
    (EXPORTING_LOCALES, $locales:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: exporting locales: {KEY_STYLE}{locales:?}{KEY_STYLE:#}", locales = $locales);
    };
    (FOUND_KEYS, $len:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: found {} keys in source code", $len);
    };
    (FOUND_UNUSED_KEYS, $len:expr, $path:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: found {} unused keys in {PATH_STYLE}{}{PATH_STYLE:#}", $len, $path);
    };
    (LIST_ITEM, $key:expr) => {
        println!("  {ERROR_STYLE}-{ERROR_STYLE:#} {KEY_STYLE}{}{KEY_STYLE:#}", $key);
    };
    (LOADED_TRS, $len:expr, $locale:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: loaded {} translations for {KEY_STYLE}{}{KEY_STYLE:#}", $len, $locale);
    };
    (LOADING, $path:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: loading {PATH_STYLE}{}{PATH_STYLE:#} ...", $path);
    };
    (LOADING_LOCALES, $path:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: loading locales from {PATH_STYLE}{}{PATH_STYLE:#} ...", $path);
    };
    (SCANNING, $root:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: scanning locales in {PATH_STYLE}{}{PATH_STYLE:#} ...", $root);
    };
    (SORTED_TO, $path:expr) => {
        println!("{IDENT_STYLE}rust-i18n{IDENT_STYLE:#}: sorted to {PATH_STYLE}{}{PATH_STYLE:#}", $path);
    };
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
    resolve: Option<MetadataResolve>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    source: Option<String>,
    manifest_path: String,
}

#[derive(Debug, Deserialize)]
struct MetadataResolve {
    root: Option<String>,
    nodes: Vec<MetadataNode>,
}

#[derive(Debug, Deserialize)]
struct MetadataNode {
    id: String,
    dependencies: Vec<String>,
}

struct DependencyScanOptions {
    include_registry: bool,
    excluded_packages: HashSet<String>,
    included_packages: Option<HashSet<String>>,
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

fn path_key(path: &Path) -> String {
    let normalized = path
        .normalize()
        .map(|p| p.into_path_buf())
        .unwrap_or_else(|_| path.to_path_buf());
    let mut key = normalized.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key.make_ascii_lowercase();
    }
    key
}

fn manifest_declares_workspace(manifest_path: &Path) -> bool {
    fs::read_to_string(manifest_path)
        .map(|contents| contents.contains("[workspace]"))
        .unwrap_or(false)
}

fn run_cargo_metadata(manifest_path: &Path) -> Result<CargoMetadata, Error> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest_dir = manifest_path.parent().ok_or_else(|| {
        anyhow::anyhow!("missing manifest directory for {}", manifest_path.display())
    })?;
    let output = Command::new(cargo)
        .current_dir(manifest_dir)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow::anyhow!(
            "failed to resolve Cargo metadata for {}: {}",
            manifest_path.display(),
            stderr
        ));
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

fn seed_package_ids(
    metadata: &CargoMetadata,
    source_manifest_path: &Path,
    is_workspace_manifest: bool,
) -> Vec<String> {
    if is_workspace_manifest && !metadata.workspace_members.is_empty() {
        return metadata.workspace_members.clone();
    }

    let manifest_key = path_key(source_manifest_path);
    if let Some(package) = metadata
        .packages
        .iter()
        .find(|package| path_key(Path::new(&package.manifest_path)) == manifest_key)
    {
        return vec![package.id.clone()];
    }

    if let Some(root) = metadata
        .resolve
        .as_ref()
        .and_then(|resolve| resolve.root.clone())
    {
        return vec![root];
    }

    metadata.workspace_members.clone()
}

fn collect_reachable_ids(
    seeds: &HashSet<String>,
    graph: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    let mut visited = HashSet::new();

    while let Some(package_id) = queue.pop_front() {
        if !visited.insert(package_id.clone()) {
            continue;
        }

        if let Some(dependencies) = graph.get(&package_id) {
            queue.extend(dependencies.iter().cloned());
        }
    }

    visited
}

fn packages_reaching_targets(
    graph: &HashMap<String, Vec<String>>,
    targets: &HashSet<String>,
) -> HashSet<String> {
    let mut reverse_graph: HashMap<String, Vec<String>> = HashMap::new();
    for (package_id, dependencies) in graph {
        reverse_graph.entry(package_id.clone()).or_default();
        for dependency in dependencies {
            reverse_graph
                .entry(dependency.clone())
                .or_default()
                .push(package_id.clone());
        }
    }

    collect_reachable_ids(targets, &reverse_graph)
}

fn is_local_package(package: &MetadataPackage) -> bool {
    package.source.is_none()
        || matches!(package.source.as_deref(), Some(source) if source.starts_with("path+"))
}

fn should_scan_package(package: &MetadataPackage, options: &DependencyScanOptions) -> bool {
    package.name != "rust-i18n"
        && !options.excluded_packages.contains(&package.name)
        && (options.include_registry || is_local_package(package))
        && !matches!(package.source.as_deref(), Some(source) if source.starts_with("git+"))
}

fn dependency_name_is_included(package: &MetadataPackage, options: &DependencyScanOptions) -> bool {
    options
        .included_packages
        .as_ref()
        .map(|packages| packages.contains(&package.name))
        .unwrap_or(true)
}

fn collect_scan_package_ids(
    metadata: &CargoMetadata,
    seed_ids: &HashSet<String>,
    options: &DependencyScanOptions,
) -> HashSet<String> {
    let Some(resolve) = metadata.resolve.as_ref() else {
        return seed_ids.clone();
    };

    let graph: HashMap<String, Vec<String>> = resolve
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.dependencies.clone()))
        .collect();

    let reachable_ids = collect_reachable_ids(seed_ids, &graph);
    let rust_i18n_ids: HashSet<String> = metadata
        .packages
        .iter()
        .filter(|package| package.name == "rust-i18n")
        .map(|package| package.id.clone())
        .collect();
    let dependent_ids = if rust_i18n_ids.is_empty() {
        HashSet::new()
    } else {
        packages_reaching_targets(&graph, &rust_i18n_ids)
    };

    metadata
        .packages
        .iter()
        .filter(|package| reachable_ids.contains(&package.id))
        .filter(|package| should_scan_package(package, options))
        .filter(|package| {
            seed_ids.contains(&package.id)
                || (dependent_ids.contains(&package.id)
                    && dependency_name_is_included(package, options))
        })
        .map(|package| package.id.clone())
        .collect()
}

fn discover_scan_roots(
    source_path: &str,
    include_deps: bool,
    options: &DependencyScanOptions,
) -> Result<Vec<PathBuf>, Error> {
    if !include_deps {
        return Ok(vec![PathBuf::from(source_path)]);
    }

    let source_root = PathBuf::from(source_path);
    let source_manifest_path = source_root.join("Cargo.toml");
    let metadata = run_cargo_metadata(&source_manifest_path)?;
    let seed_ids = seed_package_ids(
        &metadata,
        &source_manifest_path,
        manifest_declares_workspace(&source_manifest_path),
    );
    let seed_ids: HashSet<String> = seed_ids.into_iter().collect();
    let scan_ids = collect_scan_package_ids(&metadata, &seed_ids, options);

    let mut seen = HashSet::new();
    let mut scan_roots = Vec::new();
    for package in metadata.packages {
        if !scan_ids.contains(&package.id) {
            continue;
        }

        let Some(root) = Path::new(&package.manifest_path).parent() else {
            continue;
        };
        let root = root
            .normalize()
            .map(|p| p.into_path_buf())
            .unwrap_or_else(|_| root.to_path_buf());
        let key = path_key(&root);
        if seen.insert(key) {
            scan_roots.push(root);
        }
    }

    if scan_roots.is_empty() {
        scan_roots.push(source_root);
    }

    Ok(scan_roots)
}

fn i18n(args: I18nArgs) -> Result<(), Error> {
    let mut results = HashMap::new();

    let source_path = args.source.expect("Missing source path");
    let include_deps = args.include_deps;
    let included_packages = if args.include_package.is_empty() {
        None
    } else {
        Some(args.include_package.into_iter().collect())
    };
    let scan_options = DependencyScanOptions {
        include_registry: !args.local_deps_only,
        excluded_packages: args.exclude_package.into_iter().collect(),
        included_packages,
    };
    let scan_roots = discover_scan_roots(&source_path, include_deps, &scan_options)?;

    if include_deps {
        let scope = if scan_options.include_registry {
            "including registry packages"
        } else {
            "local packages only"
        };
        let exclude_hint = if scan_options.excluded_packages.is_empty() {
            String::new()
        } else {
            let mut excluded: Vec<_> = scan_options.excluded_packages.iter().cloned().collect();
            excluded.sort();
            format!("; excluding packages: {}", excluded.join(", "))
        };
        let include_hint = if let Some(included_packages) = &scan_options.included_packages {
            let mut included: Vec<_> = included_packages.iter().cloned().collect();
            included.sort();
            format!("; including only packages: {}", included.join(", "))
        } else {
            String::new()
        };
        msg!(WARN, format!(
            "--include-deps is enabled ({scope}); scanning Cargo dependencies may be slower and may merge third-party texts into the root locales output{include_hint}{exclude_hint}"
        ));
    }

    let cfg = I18nConfig::load(std::path::Path::new(&source_path))?;

    for scan_root in scan_roots {
        let scan_root = scan_root.to_string_lossy().to_string();
        iter::iter_crate(&scan_root, |path, source| {
            extractor::extract(&mut results, path, source, cfg.clone())
        })?;
    }

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

    msg!(LOADING_LOCALES, load_path_str);

    let tmp_trs = rust_i18n_support::load_locales(&load_path_str, |_| false);
    for (locale, trs) in tmp_trs.iter() {
        msg!(LOADED_TRS, trs.len(), locale);
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

    msg!(EXPORTING_LOCALES, sorted_locales);

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

    msg!(EXPORTED_TO, new_path.display());

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

fn i18n_lint(args: I18nLintArgs) -> Result<(), Error> {
    let root = args
        .manifest_dir
        .ok_or(anyhow::anyhow!("missing manifest directory"))?;
    let config = I18nConfig::load(Path::new(&root))?;
    let locales_path = find_load_path(&root, &config)?;
    let path_pattern = format!("{}/**/*.{{yml,yaml,json,toml}}", locales_path.display());

    msg!(SCANNING, root);

    let mut extrated = HashMap::new();
    iter::iter_crate(&root, |path, source| {
        extractor::extract(&mut extrated, path, source, config.clone())
    })?;

    let keys = extrated.keys().collect::<HashSet<_>>();
    msg!(FOUND_KEYS, keys.len());

    for entry in globwalk::glob(path_pattern)? {
        let entry = entry.unwrap().into_path();

        msg!(LOADING, entry.display());

        let tmp_trs = rust_i18n_support::load_locale(&entry);
        match args.lints {
            Lints::Unused => {
                let keys: HashSet<_> = tmp_trs.iter().flat_map(|(_, map)| map.keys()).collect();
                let unused_keys = keys
                    .into_iter()
                    .filter(|key| !extrated.contains_key(*key))
                    .collect::<HashSet<_>>();
                msg!(FOUND_UNUSED_KEYS, unused_keys.len(), entry.display());
                for key in unused_keys {
                    msg!(LIST_ITEM, key);
                }
            }
        }
    }

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

        msg!(LOADING, entry.display());

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
        msg!(SORTED_TO, &new_path);
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    let result = match CargoCli::parse() {
        CargoCli::I18n(args) => match args.cmd {
            Some(cmd) => match cmd {
                Commands::Export(args) => i18n_export(args),
                Commands::Lint(args) => i18n_lint(args),
                Commands::Sort(args) => i18n_sort(args),
            },
            None => i18n(args),
        },
    };

    if let Err(err) = result {
        msg!(ERROR, err);
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(id: &str, name: &str, source: Option<&str>, manifest_path: &str) -> MetadataPackage {
        MetadataPackage {
            id: id.to_string(),
            name: name.to_string(),
            source: source.map(ToOwned::to_owned),
            manifest_path: manifest_path.to_string(),
        }
    }

    fn node(id: &str, dependencies: &[&str]) -> MetadataNode {
        MetadataNode {
            id: id.to_string(),
            dependencies: dependencies
                .iter()
                .map(|dependency| dependency.to_string())
                .collect(),
        }
    }

    #[test]
    fn test_collect_scan_package_ids_filters_to_rust_i18n_dependents() {
        let options = DependencyScanOptions {
            include_registry: true,
            excluded_packages: HashSet::new(),
            included_packages: None,
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("app", "app", None, "/repo/app/Cargo.toml"),
                package("shared", "shared", None, "/repo/shared/Cargo.toml"),
                package(
                    "serde",
                    "serde",
                    Some("registry+https://example.invalid"),
                    "/registry/serde/Cargo.toml",
                ),
                package(
                    "rust-i18n",
                    "rust-i18n",
                    Some("registry+https://example.invalid"),
                    "/registry/rust-i18n/Cargo.toml",
                ),
            ],
            resolve: Some(MetadataResolve {
                root: Some("app".to_string()),
                nodes: vec![
                    node("app", &["shared", "serde"]),
                    node("shared", &["rust-i18n"]),
                    node("serde", &[]),
                    node("rust-i18n", &[]),
                ],
            }),
            workspace_members: vec!["app".to_string()],
        };

        let seed_ids = HashSet::from(["app".to_string()]);
        let package_ids = collect_scan_package_ids(&metadata, &seed_ids, &options);

        assert!(package_ids.contains("app"));
        assert!(package_ids.contains("shared"));
        assert!(!package_ids.contains("serde"));
        assert!(!package_ids.contains("rust-i18n"));
    }

    #[test]
    fn test_collect_scan_package_ids_excludes_git_dependencies() {
        let options = DependencyScanOptions {
            include_registry: true,
            excluded_packages: HashSet::new(),
            included_packages: None,
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("app", "app", None, "/repo/app/Cargo.toml"),
                package(
                    "helper",
                    "helper",
                    Some("git+https://example.invalid/repo"),
                    "/git/helper/Cargo.toml",
                ),
                package(
                    "rust-i18n",
                    "rust-i18n",
                    Some("registry+https://example.invalid"),
                    "/registry/rust-i18n/Cargo.toml",
                ),
            ],
            resolve: Some(MetadataResolve {
                root: Some("app".to_string()),
                nodes: vec![
                    node("app", &["helper"]),
                    node("helper", &["rust-i18n"]),
                    node("rust-i18n", &[]),
                ],
            }),
            workspace_members: vec!["app".to_string()],
        };

        let seed_ids = HashSet::from(["app".to_string()]);
        let package_ids = collect_scan_package_ids(&metadata, &seed_ids, &options);

        assert!(package_ids.contains("app"));
        assert!(!package_ids.contains("helper"));
    }

    #[test]
    fn test_collect_scan_package_ids_skips_registry_when_local_only() {
        let options = DependencyScanOptions {
            include_registry: false,
            excluded_packages: HashSet::new(),
            included_packages: None,
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("app", "app", None, "/repo/app/Cargo.toml"),
                package(
                    "registry-helper",
                    "registry-helper",
                    Some("registry+https://example.invalid"),
                    "/registry/helper/Cargo.toml",
                ),
                package(
                    "rust-i18n",
                    "rust-i18n",
                    Some("registry+https://example.invalid"),
                    "/registry/rust-i18n/Cargo.toml",
                ),
            ],
            resolve: Some(MetadataResolve {
                root: Some("app".to_string()),
                nodes: vec![
                    node("app", &["registry-helper"]),
                    node("registry-helper", &["rust-i18n"]),
                    node("rust-i18n", &[]),
                ],
            }),
            workspace_members: vec!["app".to_string()],
        };

        let seed_ids = HashSet::from(["app".to_string()]);
        let package_ids = collect_scan_package_ids(&metadata, &seed_ids, &options);

        assert!(package_ids.contains("app"));
        assert!(!package_ids.contains("registry-helper"));
    }

    #[test]
    fn test_collect_scan_package_ids_excludes_named_packages() {
        let options = DependencyScanOptions {
            include_registry: true,
            excluded_packages: HashSet::from(["shared".to_string()]),
            included_packages: None,
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("app", "app", None, "/repo/app/Cargo.toml"),
                package("shared", "shared", None, "/repo/shared/Cargo.toml"),
                package(
                    "rust-i18n",
                    "rust-i18n",
                    Some("registry+https://example.invalid"),
                    "/registry/rust-i18n/Cargo.toml",
                ),
            ],
            resolve: Some(MetadataResolve {
                root: Some("app".to_string()),
                nodes: vec![
                    node("app", &["shared"]),
                    node("shared", &["rust-i18n"]),
                    node("rust-i18n", &[]),
                ],
            }),
            workspace_members: vec!["app".to_string()],
        };

        let seed_ids = HashSet::from(["app".to_string()]);
        let package_ids = collect_scan_package_ids(&metadata, &seed_ids, &options);

        assert!(package_ids.contains("app"));
        assert!(!package_ids.contains("shared"));
    }

    #[test]
    fn test_collect_scan_package_ids_includes_only_named_packages() {
        let options = DependencyScanOptions {
            include_registry: true,
            excluded_packages: HashSet::new(),
            included_packages: Some(HashSet::from(["shared".to_string()])),
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("app", "app", None, "/repo/app/Cargo.toml"),
                package("shared", "shared", None, "/repo/shared/Cargo.toml"),
                package("other", "other", None, "/repo/other/Cargo.toml"),
                package(
                    "rust-i18n",
                    "rust-i18n",
                    Some("registry+https://example.invalid"),
                    "/registry/rust-i18n/Cargo.toml",
                ),
            ],
            resolve: Some(MetadataResolve {
                root: Some("app".to_string()),
                nodes: vec![
                    node("app", &["shared", "other"]),
                    node("shared", &["rust-i18n"]),
                    node("other", &["rust-i18n"]),
                    node("rust-i18n", &[]),
                ],
            }),
            workspace_members: vec!["app".to_string()],
        };

        let seed_ids = HashSet::from(["app".to_string()]);
        let package_ids = collect_scan_package_ids(&metadata, &seed_ids, &options);

        assert!(package_ids.contains("app"));
        assert!(package_ids.contains("shared"));
        assert!(!package_ids.contains("other"));
    }

    #[test]
    fn test_seed_package_ids_uses_workspace_members_for_workspace_manifest() {
        let metadata = CargoMetadata {
            packages: vec![package("root", "root", None, "/repo/Cargo.toml")],
            resolve: Some(MetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![node("root", &[])],
            }),
            workspace_members: vec!["root".to_string(), "member".to_string()],
        };

        let package_ids = seed_package_ids(&metadata, Path::new("/repo/Cargo.toml"), true);

        assert_eq!(package_ids, vec!["root".to_string(), "member".to_string()]);
    }
}
