use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    std::env::temp_dir().join(format!("rust-i18n-{prefix}-{nanos}"))
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create parent directories");
    }
    fs::write(path, contents).expect("failed to write file");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("failed to resolve repo root")
        .to_path_buf()
}

fn build_fixture() -> PathBuf {
    let fixture_root = unique_temp_dir("include-deps");
    let app_root = fixture_root.join("app");
    let dep_root = fixture_root.join("external-dep");
    let rust_i18n_root = repo_root().to_string_lossy().replace('\\', "/");

    write_file(
        &app_root.join("Cargo.toml"),
        &format!(
            r#"[package]
name = "fixture-app"
version = "0.1.0"
edition = "2021"

[dependencies]
external-dep = {{ path = "../external-dep" }}
rust-i18n = {{ path = "{rust_i18n_root}" }}
"#
        ),
    );
    write_file(
        &app_root.join("src/main.rs"),
        r#"use rust_i18n::t;

fn main() {
    let _ = t!("root.message");
    let _ = my_component::dependency_message();
}
"#,
    );

    write_file(
        &dep_root.join("Cargo.toml"),
        &format!(
            r#"[package]
name = "external-dep"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
rust-i18n = {{ path = "{rust_i18n_root}" }}
"#
        ),
    );
    write_file(
        &dep_root.join("src/lib.rs"),
        r#"use rust_i18n::t;

pub fn dependency_message() -> &'static str {
    t!("dependency.message")
}
"#,
    );

    app_root
}

fn build_registry_fixture() -> PathBuf {
    let fixture_root = unique_temp_dir("include-registry-deps");
    let app_root = fixture_root.join("app");
    let vendor_root = fixture_root.join("vendor");
    let registry_dep_root = vendor_root.join("external-registry-dep-0.1.0");
    let registry_i18n_root = vendor_root.join("rust-i18n-0.1.0");

    write_file(
        &app_root.join("Cargo.toml"),
        r#"[package]
name = "fixture-registry-app"
version = "0.1.0"
edition = "2021"

[dependencies]
external-registry-dep = "0.1.0"
rust-i18n = "0.1.0"
"#,
    );
    write_file(
        &app_root.join(".cargo/config.toml"),
        r#"[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "../vendor"
"#,
    );
    write_file(
        &app_root.join("src/main.rs"),
        r#"fn main() {
    let _ = rust_i18n::t!("root.registry-message");
    let _ = external_registry_dep::dependency_message();
}
"#,
    );

    write_file(
        &registry_dep_root.join("Cargo.toml"),
        r#"[package]
name = "external-registry-dep"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
rust-i18n = "0.1.0"
"#,
    );
    write_file(
        &registry_dep_root.join("src/lib.rs"),
        r#"pub fn dependency_message() -> &'static str {
    rust_i18n::t!("registry.message")
}
"#,
    );
    write_file(
        &registry_dep_root.join(".cargo-checksum.json"),
        r#"{"files":{},"package":null}"#,
    );

    write_file(
        &registry_i18n_root.join("Cargo.toml"),
        r#"[package]
name = "rust-i18n"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    );
    write_file(
        &registry_i18n_root.join("src/lib.rs"),
        r#"#[macro_export]
macro_rules! t {
    ($($tt:tt)*) => {
        "dummy"
    };
}
"#,
    );
    write_file(
        &registry_i18n_root.join(".cargo-checksum.json"),
        r#"{"files":{},"package":null}"#,
    );

    app_root
}

fn cleanup_fixture(app_root: &Path) {
    fs::remove_dir_all(app_root.parent().expect("fixture root missing"))
        .expect("failed to remove fixture root");
}

fn run_i18n(app_root: &Path, args: &[&str]) -> std::process::Output {
    let binary = env!("CARGO_BIN_EXE_cargo-i18n");
    let mut command = Command::new(binary);
    command.arg("i18n");
    command.args(args);
    command.arg("--");
    command.arg(app_root);
    command.output().expect("failed to run cargo-i18n binary")
}

fn build_extend_fixture() -> PathBuf {
    let app_root = build_fixture();
    let manifest =
        fs::read_to_string(app_root.join("Cargo.toml")).expect("expected fixture manifest");
    write_file(
        &app_root.join("Cargo.toml"),
        &manifest.replace(
            "external-dep = { path = \"../external-dep\" }",
            "my-component = { package = \"external-dep\", path = \"../external-dep\" }",
        ),
    );
    write_file(
        &app_root.join("src/main.rs"),
        r#"use rust_i18n::t;

fn main() {
    rust_i18n::extend!(my_component);
    let _ = t!("root.message");
    let _ = external_dep::dependency_message();
}
"#,
    );
    app_root
}

#[test]
fn extracts_extend_dependency_under_its_alias_namespace() {
    let app_root = build_extend_fixture();

    let output = run_i18n(&app_root, &["--include-deps"]);

    assert_eq!(output.status.code(), Some(1));
    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(todo.contains("my_component.dependency.message:"));
    assert!(!todo.contains("\ndependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn ignores_path_dependencies_without_include_deps() {
    let app_root = build_fixture();

    let output = run_i18n(&app_root, &[]);

    assert_eq!(output.status.code(), Some(1));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(!todo.contains("dependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn includes_path_dependencies_with_include_deps() {
    let app_root = build_fixture();

    let output = run_i18n(&app_root, &["--include-deps"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--include-deps is enabled"));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(todo.contains("dependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn excludes_named_dependencies_when_requested() {
    let app_root = build_fixture();

    let output = run_i18n(
        &app_root,
        &["--include-deps", "--exclude-package", "external-dep"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("excluding packages: external-dep"));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(!todo.contains("dependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn includes_only_named_dependencies_when_requested() {
    let app_root = build_fixture();

    let output = run_i18n(
        &app_root,
        &["--include-deps", "--include-package", "external-dep"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("including only packages: external-dep"));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(todo.contains("dependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn skips_unlisted_dependencies_when_include_package_is_used() {
    let app_root = build_fixture();

    let output = run_i18n(
        &app_root,
        &["--include-deps", "--include-package", "another-dep"],
    );

    assert_eq!(output.status.code(), Some(1));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.message:"));
    assert!(!todo.contains("dependency.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn includes_registry_dependencies_with_include_deps() {
    let app_root = build_registry_fixture();

    let output = run_i18n(&app_root, &["--include-deps"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("including registry packages"));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.registry-message:"));
    assert!(todo.contains("registry.message:"));

    cleanup_fixture(&app_root);
}

#[test]
fn skips_registry_dependencies_with_local_deps_only() {
    let app_root = build_registry_fixture();

    let output = run_i18n(&app_root, &["--include-deps", "--local-deps-only"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("local packages only"));

    let todo = fs::read_to_string(app_root.join("locales/TODO.yml"))
        .expect("expected TODO.yml to be generated");
    assert!(todo.contains("root.registry-message:"));
    assert!(!todo.contains("registry.message:"));

    cleanup_fixture(&app_root);
}
