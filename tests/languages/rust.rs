use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use constants::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and installation for Rust hooks.
#[test]
#[allow(clippy::unnecessary_wraps)]
fn language_version() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other rust versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-stable
                name: rust-stable
                language: rust
                entry: rustc --version
                language_version: 'stable'
                pass_filenames: false
                always_run: true
              - id: rust-1.70
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    let rust_dir = context.home_dir().child("tools").child("rust");
    rust_dir.assert(predicates::path::missing());

    let filters = [
        (r"rustc (1\.70)\.\d{1,2}", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2}", "rustc 1.X"), // Others become 1.X
        (r"\([a-f0-9]+ \d{4}-\d{2}-\d{2}\)", ""),  // Remove commit hash and date
        (r"  info: .*\n", ""),                     // Remove rustup info lines
        (r" +\n", "\n"),                           // Remove trailing whitespace from lines
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-stable..............................................................Passed
    - hook id: rust-stable
    - duration: [TIME]
      rustc 1.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]
      rustc 1.70.X

    ----- stderr -----
    "#);

    Ok(())
}

/// Test that `additional_dependencies` with cli: prefix are installed correctly.
#[test]
#[allow(clippy::unnecessary_wraps)]
fn additional_dependencies_cli() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rust-cli
                name: rust-cli
                language: rust
                entry: cargo-audit --version
                additional_dependencies: ["cli:cargo-audit"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-cli.................................................................Passed
    - hook id: rust-cli
    - duration: [TIME]
      cargo-audit 0.21.2

    ----- stderr -----
    ");

    Ok(())
}

/// Test that a local Rust project with library dependencies can be installed.
#[test]
fn local_with_lib_deps() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.work_dir().child("src").create_dir_all()?;
    context
        .work_dir()
        .child("Cargo.toml")
        .write_str(indoc::indoc! {r#"
        [package]
        name = "test-hook"
        version = "0.1.0"
        edition = "2021"

        [[bin]]
        name = "test-hook"
        path = "src/main.rs"
    "#})?;
    context
        .work_dir()
        .child("src/main.rs")
        .write_str(indoc::indoc! {r#"
        use upon::Engine;

        fn main() {
            let engine = Engine::new();
            engine.add_template("hello", "Hello {{ user.name }}!")?;
            let value = engine
                .template("hello")
                .render(upon::value!{ user: { name: "world" }})
                .to_string()?;
            assert_eq!(value, "Hello world!");
            println!("{}", value);
        }
    "#})?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: test-hook
                name: test-hook
                language: rust
                entry: test-hook
                additional_dependencies: ["upon"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    test-hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]
      Hello world!

    ----- stderr -----
    "#);

    Ok(())
}

/// Test that system Rust can be used.
#[test]
#[allow(clippy::unnecessary_wraps)]
fn system_rust() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: system
                name: system
                language: rust
                entry: rustc --version
                language_version: system
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    system...................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}
